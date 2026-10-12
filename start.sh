#!/bin/bash
#
# Build Signal in release mode and start it.
#
#   ./start.sh                      build dist/Signal.app (release) and open it;
#                                   the app reopens the last project from its Setup
#   ./start.sh projects/kick        ... and open this project folder
#   ./start.sh --cli projects/kick  play the project in this terminal instead
#                                   (signal play --watch; press Enter to stop)
#   ./start.sh --debug ...          debug build instead of release
#   ./start.sh --no-build ...       start the existing dist/Signal.app as it is
#
# A running Signal is asked to quit first, so the fresh build is what starts.
# Run ./install.sh once on a new Mac (toolchain and dependencies); this script
# builds offline and says so if something is missing. Written for macOS's
# /bin/bash 3.2.
set -euo pipefail

say () { printf '\033[36m==>\033[0m %s\n' "$*"; }
die () { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

PROFILE=release
BUILD=1
CLI=0
PROJECT=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug) PROFILE=debug ;;
    --no-build) BUILD=0 ;;
    --cli) CLI=1 ;;
    -h|--help) sed -n '3,16p' "$0"; exit 0 ;;
    -*) die "unknown option: $1 (see ./start.sh --help)" ;;
    *) [[ -z $PROJECT ]] || die "give one project folder, not several"; PROJECT=$1 ;;
  esac
  shift
done

[[ $(uname -s) == Darwin ]] || die "Signal runs on macOS"
ROOT=$(cd "$(dirname "$0")" && pwd)
cd "$ROOT"
APP=$ROOT/dist/Signal.app
BUNDLE_ID=hu.d250.signal

if [[ -n $PROJECT ]]; then
  [[ -f $PROJECT/project.toml ]] || die "$PROJECT has no project.toml"
  PROJECT=$(cd "$PROJECT" && pwd)
fi
[[ $CLI == 0 || -n $PROJECT ]] || die "--cli needs a project folder, e.g. ./start.sh --cli projects/kick"

# --- build -------------------------------------------------------------------
if [[ $BUILD == 1 ]]; then
  # Homebrew's rustup is keg-only: its cargo is not on PATH by default.
  if ! command -v cargo >/dev/null && [[ -x /opt/homebrew/opt/rustup/bin/cargo ]]; then
    export PATH=/opt/homebrew/opt/rustup/bin:$PATH
  fi
  command -v cargo >/dev/null || die "cargo not found: run ./install.sh first"

  BUNDLE_ARGS=()
  [[ $PROFILE == release ]] && BUNDLE_ARGS+=(--release)
  say "building Signal.app ($PROFILE)"
  scripts/bundle-app.sh ${BUNDLE_ARGS[@]+"${BUNDLE_ARGS[@]}"} \
    || die "the build failed (offline). If dependencies changed, run ./install.sh"
fi
[[ -x $APP/Contents/MacOS/Signal ]] || die "$APP is missing: run ./start.sh without --no-build"

# --- start -------------------------------------------------------------------
if [[ $CLI == 1 ]]; then
  say "playing $PROJECT (live reload on; press Enter to stop)"
  exec "$APP/Contents/Helpers/signal" play "$PROJECT" --watch
fi

if pgrep -f "$APP/Contents/MacOS/Signal" >/dev/null; then
  say "asking the running Signal to quit"
  osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    pgrep -f "$APP/Contents/MacOS/Signal" >/dev/null || break
    sleep 0.5
  done
  ! pgrep -f "$APP/Contents/MacOS/Signal" >/dev/null || die "Signal is still running: quit it and run ./start.sh again"
fi

if [[ -n $PROJECT ]]; then
  say "starting Signal with $PROJECT"
  open "$APP" --args --project "$PROJECT"
else
  say "starting Signal"
  open "$APP"
fi
