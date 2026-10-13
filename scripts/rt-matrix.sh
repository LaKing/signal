#!/bin/bash
#
# The real-time wake-policy matrix (M2.7-1): `signal play` on the default
# output device with silent copies of `signal bench --write-voices N`
# projects, for a fixed number of seconds per cell, over policies × threads ×
# voices × computation budgets. Writes cells.csv, table.md and each cell's
# JSON and log to the output folder. Unattended: no prompts, stdin closed.
#
#   scripts/rt-matrix.sh                       the default matrix (below), CoreAudio
#   scripts/rt-matrix.sh --driver null ...     the null driver: shape only, SANDBOXED
#   scripts/rt-matrix.sh --estimate            print the matrix and its duration, run nothing
#   --policies LIST      default park50,budget-spin,semaphore,fanout
#   --threads LIST       default 8,12,14 (threads per block, the audio thread included)
#   --voices LIST        default 80,120
#   --computation LIST   default 0.25,0.75 (fractions of the period, more than 0, at most 1)
#   --seconds S          default 10, per cell
#   --buffer FRAMES      default 128
#   --settle S           default 1: the pause after each cell
#   --out DIR            default a new temporary folder
#   --signal PATH        use this signal binary; else build signal-cli --release
#                        --locked --offline (as scripts/soak.sh does)
#   --target-dir DIR     build in DIR
#
# Duration: cells × (seconds + overhead), the overhead per cell being 3 s
# (process start and project load, device open and the workers entering real
# time, at most 1 s, stop, at most 1 s) plus the settle. The default matrix is
# 4 × 3 × 2 × 2 = 48 cells × (10 s + 4 s) = 672 s, 11.2 minutes; the build
# and the voice projects come before it. The script prints the estimate first.
#
# The rate is the project's (96 kHz). Every cell is labelled `device` with
# CoreAudio and `sandboxed` with the null driver: sandboxed numbers are shape
# only and never decide a real-time default (docs/benchmarks.md). Markdown
# rows of budget-spin at a computation of 0.75 or more are flagged: there it
# spins most of a period, close to the spin through the period that broke up
# the stream on the device in M2.6-5. Exit 0 when every cell ran, 1 when one
# failed (recorded as `error`), 2 when the matrix could not run. Written for
# macOS's /bin/bash 3.2; no jq or Python.
set -euo pipefail

die () { printf 'rt-matrix: %s\n' "$*" >&2; exit 2; }

DRIVER=coreaudio
POLICIES=park50,budget-spin,semaphore,fanout
THREADS=8,12,14
VOICES=80,120
COMPUTATION=0.25,0.75
CELL_SECONDS=10
BUFFER=128
SETTLE=1
OUT=
SIGNAL=
TARGET_DIR=
ESTIMATE=
# Seconds per cell beyond its playing time, before the settle.
OVERHEAD=3
while [[ $# -gt 0 ]]; do
  case "$1" in
    --driver) [[ $# -ge 2 ]] || die "--driver needs coreaudio or null"; DRIVER=$2; shift ;;
    --policies) [[ $# -ge 2 ]] || die "--policies needs a list"; POLICIES=$2; shift ;;
    --threads) [[ $# -ge 2 ]] || die "--threads needs a list"; THREADS=$2; shift ;;
    --voices) [[ $# -ge 2 ]] || die "--voices needs a list"; VOICES=$2; shift ;;
    --computation) [[ $# -ge 2 ]] || die "--computation needs a list"; COMPUTATION=$2; shift ;;
    --seconds) [[ $# -ge 2 ]] || die "--seconds needs a number"; CELL_SECONDS=$2; shift ;;
    --buffer) [[ $# -ge 2 ]] || die "--buffer needs frames"; BUFFER=$2; shift ;;
    --settle) [[ $# -ge 2 ]] || die "--settle needs seconds"; SETTLE=$2; shift ;;
    --out) [[ $# -ge 2 ]] || die "--out needs a directory"; OUT=$2; shift ;;
    --signal) [[ $# -ge 2 ]] || die "--signal needs a path"; SIGNAL=$2; shift ;;
    --target-dir) [[ $# -ge 2 ]] || die "--target-dir needs a directory"; TARGET_DIR=$2; shift ;;
    --estimate) ESTIMATE=1 ;;
    -h|--help) sed -n '3,37p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
  shift
done

case "$DRIVER" in
  coreaudio) LABEL=device ;;
  null) LABEL=sandboxed ;;
  *) die "--driver must be coreaudio or null, not $DRIVER" ;;
esac
number () { [[ $1 =~ ^[0-9]+([.][0-9]+)?$ ]]; }
number "$CELL_SECONDS" && awk "BEGIN { exit !($CELL_SECONDS > 0) }" \
  || die "--seconds must be a positive number, not $CELL_SECONDS"
number "$SETTLE" || die "--settle must be a number of seconds, not $SETTLE"
[[ $BUFFER =~ ^[1-9][0-9]*$ ]] || die "--buffer must be a positive whole number, not $BUFFER"

# Process substitution, not here-strings: bash 3.2 writes a here-string to a
# temporary file in /tmp, which a sandbox may deny.
IFS=, read -r -a POLICY_LIST < <(printf '%s\n' "$POLICIES")
IFS=, read -r -a THREAD_LIST < <(printf '%s\n' "$THREADS")
IFS=, read -r -a VOICE_LIST < <(printf '%s\n' "$VOICES")
IFS=, read -r -a COMPUTATION_LIST < <(printf '%s\n' "$COMPUTATION")
[[ ${#POLICY_LIST[@]} -gt 0 && ${#THREAD_LIST[@]} -gt 0 && ${#VOICE_LIST[@]} -gt 0 \
  && ${#COMPUTATION_LIST[@]} -gt 0 ]] || die "every list needs at least one value"
for p in "${POLICY_LIST[@]}"; do
  case "$p" in park50|budget-spin|semaphore|fanout) ;; *) die "unknown policy: $p" ;; esac
done
for t in "${THREAD_LIST[@]}"; do
  [[ $t =~ ^[1-9][0-9]*$ ]] || die "threads must be positive whole numbers, not $t"
done
for v in "${VOICE_LIST[@]}"; do
  [[ $v =~ ^[1-9][0-9]*$ ]] || die "voices must be positive whole numbers, not $v"
done
for c in "${COMPUTATION_LIST[@]}"; do
  number "$c" && awk "BEGIN { exit !($c > 0 && $c <= 1) }" \
    || die "a computation must be above 0 and at most 1, not $c"
done

CELLS=$(( ${#POLICY_LIST[@]} * ${#THREAD_LIST[@]} * ${#VOICE_LIST[@]} * ${#COMPUTATION_LIST[@]} ))
PER_CELL=$(awk "BEGIN { print $CELL_SECONDS + $OVERHEAD + $SETTLE }")
TOTAL=$(awk "BEGIN { print $CELLS * $PER_CELL }")
printf 'rt-matrix: %s cells (%s policies × %s thread counts × %s voice counts × %s budgets) × (%s s + %s s overhead) = %s s (%s min), %s driver, labelled %s\n' \
  "$CELLS" "${#POLICY_LIST[@]}" "${#THREAD_LIST[@]}" "${#VOICE_LIST[@]}" "${#COMPUTATION_LIST[@]}" \
  "$CELL_SECONDS" "$(awk "BEGIN { print $OVERHEAD + $SETTLE }")" "$TOTAL" \
  "$(awk "BEGIN { printf \"%.1f\", $TOTAL / 60 }")" "$DRIVER" "$LABEL" >&2
[[ -n $ESTIMATE ]] && exit 0

ROOT=$(cd "$(dirname "$0")/.." && pwd)

# --- the binary (as scripts/soak.sh) ----------------------------------------
if [[ -z $SIGNAL ]]; then
  CARGO=${CARGO:-cargo}
  if ! command -v "$CARGO" >/dev/null && [[ -x /opt/homebrew/opt/rustup/bin/cargo ]]; then
    CARGO=/opt/homebrew/opt/rustup/bin/cargo
  fi
  command -v "$CARGO" >/dev/null || die "cargo not found (or give --signal PATH)"
  BUILD=(build --release --locked --offline -p signal-cli)
  [[ -n $TARGET_DIR ]] && BUILD+=(--target-dir "$TARGET_DIR")
  (cd "$ROOT" && "$CARGO" "${BUILD[@]}") || die "the build failed"
  if [[ -z $TARGET_DIR ]]; then
    TARGET_DIR=$(cd "$ROOT" && "$CARGO" metadata --locked --offline --no-deps --format-version 1 \
      | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
    [[ -n $TARGET_DIR ]] || die "cargo metadata reported no target directory"
  fi
  SIGNAL=$TARGET_DIR/release/signal
fi
[[ -x $SIGNAL ]] || die "$SIGNAL is not an executable"

# --- the silent voice projects ----------------------------------------------
if [[ -z $OUT ]]; then
  OUT=$(mktemp -d "${TMPDIR:-/tmp}/signal-rt-matrix.XXXXXX") || die "no temporary folder"
else
  mkdir -p "$OUT" || die "cannot create $OUT"
fi
# The sink's gain becomes 0.001 (−60 dB): every module still runs (as
# scripts/soak.sh --silent and the operator's silent copies).
OUT_PATTERN='^\(out = { module = "std.audio_out", gain = \)[0-9.]*'
for v in "${VOICE_LIST[@]}"; do
  PROJECT=$OUT/voices-$v
  if [[ ! -f $PROJECT/project.toml ]]; then
    "$SIGNAL" bench --write-voices "$v" "$PROJECT" >/dev/null 2>&1 || die "writing $v voices failed"
  fi
  grep -q "$OUT_PATTERN" "$PROJECT/project.toml" || die "$PROJECT has no out gain to silence"
  sed "s/$OUT_PATTERN/\10.001/" "$PROJECT/project.toml" > "$PROJECT/.project.toml.new"
  mv "$PROJECT/.project.toml.new" "$PROJECT/project.toml"
done

# --- the cells --------------------------------------------------------------
CSV=$OUT/cells.csv
MD=$OUT/table.md
echo "label,policy,threads,voices,computation,seconds,xruns,stream_errors,overruns,worst,p99,median,wake_median_us,wake_p99_us,wake_max_us,wakes_observed,wakes_signalled,first_join_median_us,first_join_p99_us,last_join_median_us,last_join_p99_us,last_join_max_us,slowest_task_p99_us,callbacks_without_join,demotion_samples,demotions_timeshare,demotions_unsampled,waiter,scheduling,status" > "$CSV"

# From the pretty-printed JSON of `signal play --json`: a top-level key
# (indented by two spaces), and a key of a top-level object.
field () { sed -n "s/^  \"$2\": \([^,]*\),*$/\1/p" "$1" | head -n 1; }
block () { awk -v block="\"$2\": {" -v key="\"$3\":" \
  'index($0, block) { inside = 1 } inside && index($0, key) { sub(/.*: */, ""); sub(/,$/, ""); print; exit }' \
  "$1"; }

FAILED=0
N=0
for c in "${COMPUTATION_LIST[@]}"; do
  for t in "${THREAD_LIST[@]}"; do
    for v in "${VOICE_LIST[@]}"; do
      for p in "${POLICY_LIST[@]}"; do
        N=$((N + 1))
        NAME=$p-t$t-v$v-c$c
        JSON=$OUT/cell-$NAME.json
        LOG=$OUT/cell-$NAME.log
        printf 'rt-matrix: cell %s of %s: %s, %s threads, %s voices, computation %s\n' \
          "$N" "$CELLS" "$p" "$t" "$v" "$c" >&2
        STATUS=ok
        "$SIGNAL" --json play "$OUT/voices-$v" --driver "$DRIVER" --buffer "$BUFFER" \
          --seconds "$CELL_SECONDS" --threads "$t" --rt-policy "$p" --rt-computation "$c" \
          --rt-trace < /dev/null > "$JSON" 2> "$LOG" || STATUS=error
        if [[ $STATUS == ok ]] && [[ -z $(field "$JSON" worst_callback_ratio) ]]; then
          STATUS=error
        fi
        if [[ $STATUS == error ]]; then
          FAILED=1
          printf 'rt-matrix: cell %s failed; see %s\n' "$NAME" "$LOG" >&2
          echo "$LABEL,$p,$t,$v,$c,$CELL_SECONDS,,,,,,,,,,,,,,,,,,,,,,,,error" >> "$CSV"
        else
          w () { block "$JSON" wake "$1"; }
          d () { block "$JSON" rt_demotions "$1"; }
          SCHED=$(block "$JSON" scheduling summary | sed 's/^"//; s/"$//; s/"/'"'"'/g')
          WAITER=$(field "$JSON" waiter | sed 's/"//g')
          echo "$LABEL,$p,$t,$v,$c,$CELL_SECONDS,$(block "$JSON" xruns total),$(block "$JSON" xruns stream_errors),$(block "$JSON" xruns overruns),$(field "$JSON" worst_callback_ratio),$(field "$JSON" p99_callback_ratio),$(field "$JSON" median_callback_ratio),$(w wake_median_us),$(w wake_p99_us),$(w wake_max_us),$(w wakes_observed),$(w wakes_signalled),$(w first_join_median_us),$(w first_join_p99_us),$(w last_join_median_us),$(w last_join_p99_us),$(w last_join_max_us),$(w slowest_task_p99_us),$(w callbacks_without_join),$(d samples),$(d sampled_timeshare),$(d unsampled),$WAITER,\"$SCHED\",ok" >> "$CSV"
        fi
        sleep "$SETTLE"
      done
    done
  done
done

# --- the Markdown table -----------------------------------------------------
{
  if [[ $LABEL == device ]]; then
    printf '# Wake-policy matrix: device measurement\n\n'
  else
    printf '# Wake-policy matrix: SANDBOXED (null driver)\n\nShape only: no audio device, no workgroup, no real-time scheduling. These numbers never decide a real-time default.\n\n'
  fi
  printf '%s, %s s per cell, buffer %s, `%s`, %s\n\n' "$DRIVER" "$CELL_SECONDS" "$BUFFER" \
    "$("$SIGNAL" --version 2>/dev/null || echo signal)" "$(date -u '+%Y-%m-%d %H:%M UTC')"
  printf 'Callback times are fractions of the period (V12: worst < 0.25 with 0 xruns). Wake components in µs: wake = from a wake to the worker running; joins from the callback'"'"'s start. ⚠ = budget-spin at a computation of 0.75 or more: it spins most of a period, near the spin through the period rejected on the device in M2.6-5.\n\n'
  printf '| Policy | Threads | Voices | Computation | Xruns | Worst | p99 | Median | Wake p50 / p99 / max | First join p50 / p99 | Last join p50 / p99 / max | Slowest task p99 | Timeshare samples | Status |\n'
  printf '|---|---:|---:|---:|---:|---:|---:|---:|---|---|---|---:|---:|---|\n'
  tail -n +2 "$CSV" | awk -F, '{
    flag = ($2 == "budget-spin" && $5 >= 0.75) ? " ⚠" : ""
    st = $NF
    if (st == "error") {
      printf "| %s%s | %s | %s | %s | | | | | | | | | | error |\n", $2, flag, $3, $4, $5
    } else {
      printf "| %s%s | %s | %s | %s | %s | %.3f | %.2f | %.2f | %s / %s / %s | %s / %s | %s / %s / %s | %s | %s of %s | %s |\n", \
        $2, flag, $3, $4, $5, $7, $10, $11, $12, $13, $14, $15, $18, $19, $20, $21, $22, $23, $26, $25, st
    }
  }'
} > "$MD"

printf 'rt-matrix: %s cells, labelled %s\n' "$CELLS" "$LABEL"
printf 'csv %s\ntable %s\n' "$CSV" "$MD"
exit $FAILED
