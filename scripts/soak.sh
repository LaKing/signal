#!/bin/bash
#
# The V12 soak: play a temporary copy of projects/kick in real time with
# `signal play --watch --json`, rewrite one parameter every 10 s (accent
# decay_ms, alternating 200 and 300), then print the xruns, the worst callback
# fraction, and PASS or FAIL against V12 and the multi-core gate (below): 0 xruns and a worst callback under
# 25 % of the period. The committed project is never edited.
#
#   scripts/soak.sh [minutes] [buffer]       default device (CoreAudio), 30 min, 128 frames
#   scripts/soak.sh --driver null 1          the null driver, 1 minute (no device)
#   scripts/soak.sh --signal PATH ...        use this signal binary, e.g.
#                                            dist/Signal.app/Contents/Helpers/signal
#   scripts/soak.sh --target-dir DIR ...     build in DIR (else CARGO_TARGET_DIR,
#                                            else .cargo/config.toml, as cargo does)
#   scripts/soak.sh --voices N ...           play the kick with N voices instead
#                                            (`signal bench --write-voices N`)
#   scripts/soak.sh --threads n ...          threads per block, the audio thread
#                                            included (default: signal play's auto)
#   scripts/soak.sh --silent ...             set the copy's `out` gain to 0.001
#                                            (−60 dB): every module still runs
#   scripts/soak.sh --rt-policy P ...        the wake policy (signal play --rt-policy)
#   scripts/soak.sh --rt-computation C ...   the workers' computation, a fraction
#                                            of the period (signal play --rt-computation)
#
# The rate is the project's (96 kHz). Without --signal it builds signal-cli
# with --release --locked --offline and takes the binary from the target
# directory cargo reports, never a fixed path. The edit is sed and mv; no
# Python. Exit 0 on PASS, 1 on FAIL, 2 when the soak could not run. Written
# for macOS's /bin/bash 3.2.
set -euo pipefail

die () { printf 'soak: %s\n' "$*" >&2; exit 2; }

DRIVER=coreaudio
SIGNAL=
TARGET_DIR=
VOICES=
THREADS=
SILENT=
RT_POLICY=
RT_COMPUTATION=
POSITIONAL=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --driver) [[ $# -ge 2 ]] || die "--driver needs coreaudio or null"; DRIVER=$2; shift ;;
    --signal) [[ $# -ge 2 ]] || die "--signal needs a path"; SIGNAL=$2; shift ;;
    --target-dir) [[ $# -ge 2 ]] || die "--target-dir needs a directory"; TARGET_DIR=$2; shift ;;
    --voices) [[ $# -ge 2 ]] || die "--voices needs a number of voices"; VOICES=$2; shift ;;
    --threads) [[ $# -ge 2 ]] || die "--threads needs a number of threads"; THREADS=$2; shift ;;
    --silent) SILENT=1 ;;
    --rt-policy) [[ $# -ge 2 ]] || die "--rt-policy needs a policy"; RT_POLICY=$2; shift ;;
    --rt-computation) [[ $# -ge 2 ]] || die "--rt-computation needs a fraction"; RT_COMPUTATION=$2; shift ;;
    -h|--help) sed -n '3,29p' "$0"; exit 0 ;;
    -*) die "unknown argument: $1" ;;
    *) POSITIONAL+=("$1") ;;
  esac
  shift
done
[[ ${#POSITIONAL[@]} -le 2 ]] || die "too many arguments: ${POSITIONAL[*]}"
MINUTES=${POSITIONAL[0]:-30}
BUFFER=${POSITIONAL[1]:-128}
case "$DRIVER" in coreaudio|null) ;; *) die "--driver must be coreaudio or null, not $DRIVER" ;; esac
[[ $MINUTES =~ ^[0-9]+([.][0-9]+)?$ ]] && awk "BEGIN { exit !($MINUTES > 0) }" \
  || die "minutes must be a positive number, not $MINUTES"
[[ $BUFFER =~ ^[1-9][0-9]*$ ]] || die "buffer must be a positive whole number of frames, not $BUFFER"
[[ -z $VOICES || $VOICES =~ ^[1-9][0-9]*$ ]] || die "--voices must be a positive whole number, not $VOICES"
[[ -z $THREADS || $THREADS =~ ^[0-9]+$ ]] || die "--threads must be a whole number, not $THREADS"
SECONDS_TOTAL=$(awk "BEGIN { print $MINUTES * 60 }")

ROOT=$(cd "$(dirname "$0")/.." && pwd)

# --- the binary ---------------------------------------------------------------
if [[ -z $SIGNAL ]]; then
  CARGO=${CARGO:-cargo}
  if ! command -v "$CARGO" >/dev/null && [[ -x /opt/homebrew/opt/rustup/bin/cargo ]]; then
    CARGO=/opt/homebrew/opt/rustup/bin/cargo
  fi
  command -v "$CARGO" >/dev/null || die "cargo not found (or give --signal PATH)"
  BUILD=(build --release --locked --offline -p signal-cli)
  [[ -n $TARGET_DIR ]] && BUILD+=(--target-dir "$TARGET_DIR")
  (cd "$ROOT" && "$CARGO" "${BUILD[@]}") || die "the build failed"
  # The directory cargo used: --target-dir, else what cargo resolves from
  # CARGO_TARGET_DIR and its config.
  if [[ -z $TARGET_DIR ]]; then
    TARGET_DIR=$(cd "$ROOT" && "$CARGO" metadata --locked --offline --no-deps --format-version 1 \
      | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
    [[ -n $TARGET_DIR ]] || die "cargo metadata reported no target directory"
  fi
  SIGNAL=$TARGET_DIR/release/signal
fi
[[ -x $SIGNAL ]] || die "$SIGNAL is not an executable"

# --- the copy -----------------------------------------------------------------
WORK=$(mktemp -d "${TMPDIR:-/tmp}/signal-soak.XXXXXX") || die "no temporary folder"
if [[ -n $VOICES ]]; then
  "$SIGNAL" bench --write-voices "$VOICES" "$WORK/kick" >&2 || die "writing $VOICES voices failed"
else
  cp -R "$ROOT/projects/kick" "$WORK/kick"
fi
PROJECT=$WORK/kick/project.toml
# --silent: the sink's gain (0.75 in projects/kick, divided by N with
# --voices) becomes 0.001, as the operator's silent copies at the M2.6-3 and
# M2.6-4 checkpoints.
if [[ -n $SILENT ]]; then
  OUT_PATTERN='^\(out = { module = "std.audio_out", gain = \)[0-9.]*'
  grep -q "$OUT_PATTERN" "$PROJECT" || die "the copy has no out gain to silence"
  sed "s/$OUT_PATTERN/\10.001/" "$PROJECT" > "$WORK/kick/.project.toml.new"
  mv "$WORK/kick/.project.toml.new" "$PROJECT"
fi
# The line rewritten: `accent = { module = "std.exp_env", decay_ms = 200 }`.
PATTERN='^\(accent = {.*decay_ms = \)[0-9.]*'
grep -q "$PATTERN" "$PROJECT" || die "projects/kick/project.toml has no accent decay_ms to edit"

# Rewrites accent decay_ms in the copy: a new file beside it, then a rename,
# so the watch never reads half a file. (A dot file: the watch ignores it.)
edit () {
  sed "s/$PATTERN/\1$1/" "$PROJECT" > "$WORK/kick/.project.toml.new"
  mv "$WORK/kick/.project.toml.new" "$PROJECT"
}

# --- the run ------------------------------------------------------------------
PLAY=(--json play "$WORK/kick" --watch --driver "$DRIVER" --buffer "$BUFFER" --seconds "$SECONDS_TOTAL")
[[ -n $THREADS ]] && PLAY+=(--threads "$THREADS")
[[ -n $RT_POLICY ]] && PLAY+=(--rt-policy "$RT_POLICY")
[[ -n $RT_COMPUTATION ]] && PLAY+=(--rt-computation "$RT_COMPUTATION")
printf 'soak: %s min, %s driver, buffer %s, voices %s, threads %s, wake policy %s, computation %s, in %s\n' \
  "$MINUTES" "$DRIVER" "$BUFFER" "${VOICES:-1}" "${THREADS:-auto}" "${RT_POLICY:-park50}" \
  "${RT_COMPUTATION:-0.25}" "$WORK" >&2
"$SIGNAL" "${PLAY[@]}" < /dev/null > "$WORK/play.json" 2> "$WORK/play.log" &
PID=$!
trap 'kill "$PID" 2>/dev/null || true' EXIT INT TERM

ELAPSED=0
EDITS=0
VALUES=(300 200)
while kill -0 "$PID" 2>/dev/null; do
  sleep 1
  ELAPSED=$((ELAPSED + 1))
  if (( ELAPSED % 10 == 0 )) && kill -0 "$PID" 2>/dev/null; then
    edit "${VALUES[$((EDITS % 2))]}"
    EDITS=$((EDITS + 1))
  fi
done
STATUS=0
wait "$PID" || STATUS=$?
trap - EXIT INT TERM
if [[ $STATUS -ne 0 ]]; then
  cat "$WORK/play.log" "$WORK/play.json" >&2
  die "signal play exited with $STATUS"
fi

# --- the verdict --------------------------------------------------------------
# From the pretty-printed JSON of `signal play --json`, top-level keys indented
# by two spaces (no jq or Python).
field () { sed -n "s/^  \"$1\": \([^,]*\),*$/\1/p" "$WORK/play.json" | head -n 1; }
block () { awk -v block="\"$1\": {" -v key="\"$2\":" \
  'index($0, block) { inside = 1 } inside && index($0, key) { sub(/.*: */, ""); sub(/,$/, ""); print; exit }' \
  "$WORK/play.json"; }
RATE=$(field sample_rate)
FRAMES=$(field frames)
WORST=$(field worst_callback_ratio)
XRUNS=$(block xruns total)
SETS=$(block edits sets)
ERRORS=$(block edits errors)
PLAYED_THREADS=$(field threads)
WORKERS=$(field workers)
SCHEDULING=$(block scheduling summary | sed 's/^"//; s/"$//')
[[ -n $XRUNS && -n $WORST && -n $SETS ]] || { cat "$WORK/play.json" >&2; die "unreadable play output"; }

printf 'soak: %s frames at %s Hz, buffer %s, %s driver\n' "$FRAMES" "$RATE" "$BUFFER" "$DRIVER"
printf 'voices %s, threads %s (%s workers), scheduling %s\n' "${VOICES:-1}" "$PLAYED_THREADS" "$WORKERS" "$SCHEDULING"
printf 'wake policy %s, computation %s of the period\n' "$(field rt_policy | sed 's/"//g')" "$(field rt_computation)"
[[ -n $SILENT ]] && printf 'silent (out gain 0.001)\n'
printf 'edits %s written, %s applied as parameter sets, %s errors\n' "$EDITS" "$SETS" "$ERRORS"
printf 'xruns %s\n' "$XRUNS"
printf 'worst callback %.3f of the period\n' "$WORST"
printf 'output %s/play.json, log %s/play.log\n' "$WORK" "$WORK"
MEDIAN=$(field median_callback_ratio)
[[ -n $MEDIAN ]] && printf 'median callback %.3f of the period; worst - median %.3f\n' "$MEDIAN" "$(awk "BEGIN { print $WORST - $MEDIAN }")"
# Two verdicts (docs/benchmarks.md "Wake-policy matrix on the device"):
# V12 is a headroom rule (0 xruns, worst < 0.25), meaningful when the median
# load is well under 0.25 (the kick); the multi-core gate (0 xruns, worst -
# median < 0.25) is the scheduling-jitter rule for loads of their own.
# The exit status follows V12 unless --voices was given, then the gate.
V12=FAIL; GATE=FAIL
if [[ $XRUNS -eq 0 ]] && awk "BEGIN { exit !($WORST < 0.25) }"; then V12=PASS; fi
if [[ $XRUNS -eq 0 && -n $MEDIAN ]] && awk "BEGIN { exit !($WORST - $MEDIAN < 0.25) }"; then GATE=PASS; fi
echo "V12 $V12 (0 xruns and worst callback < 0.25 of the period)"
echo "multi-core gate $GATE (0 xruns and worst - median < 0.25 of the period)"
if [[ -n ${VOICES:-} ]]; then [[ $GATE == PASS ]] || exit 1; else [[ $V12 == PASS ]] || exit 1; fi
