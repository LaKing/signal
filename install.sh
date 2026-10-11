#!/bin/bash
#
# Prepare this Mac to develop Signal, and prove the workspace builds offline.
#
#   ./install.sh            check, install what is missing, fetch, verify
#   ./install.sh --no-verify   skip the final fmt/clippy/test pass
#
# Idempotent: tools are installed only when absent; re-running after a
# Cargo.toml or Cargo.lock change fetches only the new crates.
#
# Why the offline verification: devpanel seats build without network, so every
# dependency must already be in the local cargo registry. Adding a dependency is
# therefore an operator step: edit [workspace.dependencies], then run this again.
#
# Runs as the developer, never with sudo. Written for macOS's /bin/bash 3.2.
set -euo pipefail

say () { printf '\033[36m==>\033[0m %s\n' "$*"; }
warn () { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
die () { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

VERIFY=1
for arg in "$@"; do
  case "$arg" in
    --no-verify) VERIFY=0 ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) die "unknown argument: $arg" ;;
  esac
done

[[ $EUID -ne 0 ]] || die "do not run as root"
cd "$(dirname "$0")"
[[ -f rust-toolchain.toml && -f Cargo.toml ]] || die "run from the Signal repository root"

# --- platform (vision C1: macOS on Apple Silicon) ---------------------------
[[ "$(uname -s)" == Darwin ]] || die "Signal targets macOS; this is $(uname -s)"
[[ "$(uname -m)" == arm64 ]] || warn "Signal targets Apple Silicon; this is $(uname -m)"
say "macOS $(sw_vers -productVersion) on $(uname -m)"

# --- Homebrew and rustup ----------------------------------------------------
command -v brew >/dev/null 2>&1 || die "Homebrew is required: https://brew.sh"
if brew list --formula rustup >/dev/null 2>&1; then
  say "rustup present"
else
  say "installing rustup (Homebrew)"
  brew install rustup
fi
# Homebrew's rustup is keg-only: its proxies (cargo, rustc, ...) are not on PATH.
RUSTUP_BIN="$(brew --prefix rustup)/bin"
export PATH="$RUSTUP_BIN:$PATH"
command -v cargo >/dev/null 2>&1 || die "cargo proxy not found in $RUSTUP_BIN"
case ":$(/usr/bin/env -i HOME="$HOME" /bin/zsh -lc 'echo $PATH' 2>/dev/null):" in
  *":$RUSTUP_BIN:"*) ;;
  *) warn "$RUSTUP_BIN is not on your login PATH; add: export PATH=\"$RUSTUP_BIN:\$PATH\"" ;;
esac

# --- pinned toolchain (rust-toolchain.toml; ADR 0007/0008) --------------------
say "toolchain from rust-toolchain.toml"
rustup toolchain install >/dev/null   # installs the pinned channel and components when missing
say "  $(rustc --version)"
say "  $(cargo --version)"
cargo fmt --version >/dev/null 2>&1 || die "rustfmt missing from the pinned toolchain"
cargo clippy --version >/dev/null 2>&1 || die "clippy missing from the pinned toolchain"

# --- optional tools -----------------------------------------------------------
if command -v cargo-nextest >/dev/null 2>&1; then
  say "cargo-nextest present (CI's test runner)"
elif brew info --formula cargo-nextest >/dev/null 2>&1; then
  say "installing cargo-nextest (Homebrew)"
  brew install cargo-nextest
else
  warn "cargo-nextest unavailable; cargo test works the same locally"
fi

# --- dependencies: fetch now, so builds work offline --------------------------
say "fetching dependencies (Cargo.lock)"
cargo fetch --locked

# --- verification, offline, exactly as devpanel seats run it ---------------------
if [[ $VERIFY -eq 1 ]]; then
  say "verifying offline: fmt, clippy (-D warnings), tests"
  cargo fmt --all --check
  cargo clippy --workspace --all-targets --locked --offline -- -D warnings
  cargo test --workspace --locked --offline
fi

say "ready. Commands (see AGENTS.md):"
say "  cargo fmt --all --check"
say "  cargo clippy --workspace --all-targets --offline -- -D warnings"
say "  cargo test --workspace --offline"
