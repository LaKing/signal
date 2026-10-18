#!/bin/bash
#
# The cost of the M2.10-4 validation pairs (docs/benchmarks.md, "Validation
# pairs"): wall time of `signal render --threads 1` of temporary projects at
# 48 kHz, each holding a noise source into `std.audio_out` and N instances of
# one native type or of its `lib.*_core` composite fed by that source (their
# outputs open: every instance still runs). The cost per instance and frame is
# (time − base time)/(N · frames), the base being the same project without
# the N instances, from the median of the runs. A native may take settings of
# its own (std.osc's shape = 0, the sine the composite rebuilds). Then, as
# aggregate figures for
# the cycle-unit breakdown, chains and loops of std.add (and std.select) at
# --block 1 and 256. Measurements on one host, never thresholds.
#
#   scripts/validation-cost.sh [seconds] [runs]   default 10 s, 3 runs
#   scripts/validation-cost.sh --signal PATH ...  use this signal binary
#   scripts/validation-cost.sh --target-dir DIR   build in DIR (else
#                                                 CARGO_TARGET_DIR, as cargo does)
#   scripts/validation-cost.sh --pairs-only ...   skip the chain and loop figures
#   scripts/validation-cost.sh --only PAIR ...    one pair only (svf, onepole,
#                                                 lag, adsr, osc_sine, delay,
#                                                 tanh_shaper), no chains
#
# Without --signal it builds signal-cli with --release --locked --offline and
# takes the binary from the target directory cargo reports. Prints one line
# per measurement. Written for macOS's /bin/bash 3.2.
set -euo pipefail

die () { printf 'validation-cost: %s\n' "$*" >&2; exit 2; }

SIGNAL=
TARGET_DIR=
PAIRS_ONLY=
ONLY=
MATCHED=
POSITIONAL=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --signal) [[ $# -ge 2 ]] || die "--signal needs a path"; SIGNAL=$2; shift ;;
    --target-dir) [[ $# -ge 2 ]] || die "--target-dir needs a directory"; TARGET_DIR=$2; shift ;;
    --pairs-only) PAIRS_ONLY=1 ;;
    --only) [[ $# -ge 2 ]] || die "--only needs a pair"; ONLY=$2; PAIRS_ONLY=1; shift ;;
    -h|--help) sed -n '3,27p' "$0"; exit 0 ;;
    -*) die "unknown argument: $1" ;;
    *) POSITIONAL+=("$1") ;;
  esac
  shift
done
SECONDS_RENDER=${POSITIONAL[0]:-10}
RUNS=${POSITIONAL[1]:-3}
RATE=48000
N=32
ROOT=$(cd "$(dirname "$0")/.." && pwd)

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

WORK=$(mktemp -d "${TMPDIR:-/tmp}/signal-validation-cost.XXXXXX") || die "no temporary folder"
trap 'rm -rf "$WORK"' EXIT

# write NAME INSTANCES CONNECTIONS: a project folder at 48 kHz, seed 1, with
# a noise source `src` and the sink `out` fed by it.
write () {
  mkdir -p "$WORK/$1"
  {
    printf 'schema_version = 1\n\n[project]\nname = "%s"\nsample_rate = %s\nseed = 1\n\n' "$1" "$RATE"
    printf '[instances]\nsrc = { module = "std.noise" }\nout = { module = "std.audio_out", gain = 0.001 }\n%s\n' "$2"
    printf '[wiring]\nconnections = [\n{ from = "src.out", to = "out.l" },\n%s]\n' "$3"
  } > "$WORK/$1/project.toml"
}

# seconds NAME BLOCK: the median wall time of RUNS renders, in seconds.
seconds () {
  local times=() i
  for ((i = 0; i < RUNS; i++)); do
    /usr/bin/time -p "$SIGNAL" render "$WORK/$1" --seconds "$SECONDS_RENDER" --threads 1 \
      --block "$2" --out "$WORK/$1/render" > /dev/null 2> "$WORK/time"
    times+=("$(awk '/^real/ { print $2 }' "$WORK/time")")
  done
  printf '%s\n' "${times[@]}" | sort -n | awk '{ t[NR] = $1 } END { print t[int((NR + 1) / 2)] }'
}

# per_instance TIME BASE COUNT: ns per instance and frame.
per_instance () {
  awk -v t="$1" -v b="$2" -v n="$3" -v s="$SECONDS_RENDER" -v r="$RATE" \
    'BEGIN { printf "%.1f", (t - b) * 1e9 / (n * s * r) }'
}

# instances MODULE PARAMETERS INPUT: N instances fed by the noise (INPUT
# empty: none), plus the gate source an envelope needs.
instances () {
  local i lines="" wires=""
  for ((i = 1; i <= N; i++)); do
    lines+="v$i = { module = \"$1\"$2 }"$'\n'
    [[ -n $3 ]] && wires+="{ from = \"$4\", to = \"v$i.$3\" },"$'\n'
  done
  INSTANCES=$lines
  WIRES=$wires
}

GATE='gate_clock = { module = "std.phasor", freq = 6 }
half = { module = "std.const", value = 0.5 }
gate = { module = "std.compare", mode = 0 }'
GATE_WIRES='{ from = "gate_clock.phase", to = "gate.a" },
{ from = "half.out", to = "gate.b" },
'

write base "" ""
write base_gate "$GATE" "$GATE_WIRES"
BASE=$(seconds base 256)
BASE_GATE=$(seconds base_gate 256)
printf 'base: %s s, base with a gate: %s s (%s s at 48 kHz, median of %s, --threads 1, --block 256, %s instances)\n' \
  "$BASE" "$BASE_GATE" "$SECONDS_RENDER" "$RUNS" "$N"

# pair NAME NATIVE COMPOSITE PARAMETERS INPUT [gate] [NATIVE_ONLY]: NATIVE_ONLY
# (e.g. ", shape = 0") is added to the native's settings only.
pair () {
  [[ -z $ONLY || $ONLY == "$1" ]] || return 0
  MATCHED=1
  local base=$BASE src=src.out extra="" extra_wires="" native composite
  if [[ ${6:-} == gate ]]; then base=$BASE_GATE; src=gate.out; extra=$GATE; extra_wires=$GATE_WIRES; fi
  instances "$2" "$4${7:-}" "$5" "$src"
  write "native_$1" "$extra"$'\n'"$INSTANCES" "$extra_wires$WIRES"
  instances "$3" "$4" "$5" "$src"
  write "composite_$1" "$extra"$'\n'"$INSTANCES" "$extra_wires$WIRES"
  native=$(per_instance "$(seconds "native_$1" 256)" "$base" "$N")
  composite=$(per_instance "$(seconds "composite_$1" 256)" "$base" "$N")
  printf 'pair %-18s native %8s ns/frame, composite %8s ns/frame, ratio %s\n' "$1" "$native" "$composite" \
    "$(awk -v a="$composite" -v b="$native" 'BEGIN { if (b > 0) printf "%.1f", a / b; else print "n/a" }')"
}

pair svf std.svf lib.svf_core "" in
pair onepole std.onepole lib.onepole_core "" in
pair lag std.lag lib.lag_core "" in
pair adsr std.adsr lib.adsr_core "" gate gate
pair osc_sine std.osc lib.osc_core "" "" "" ", shape = 0"
pair delay std.delay lib.delay_core "" in
pair tanh_shaper std.tanh_shaper lib.tanh_shaper_core ", drive = 0.5" in

[[ -z $ONLY || -n $MATCHED ]] || die "no pair named $ONLY"
[[ -n $PAIRS_ONLY ]] && exit 0

# chain MODULE COUNT LOOP: COUNT instances of MODULE in a chain from the
# noise (port a, and b for select's gate), closed into a loop through one
# std.unit_delay when LOOP is 1.
chain () {
  local i lines="" wires="" prev=src.out
  for ((i = 1; i <= $2; i++)); do
    lines+="c$i = { module = \"$1\" }"$'\n'
    wires+="{ from = \"$prev\", to = \"c$i.a\" },"$'\n'
    [[ $1 == std.select ]] && wires+="{ from = \"src.out\", to = \"c$i.gate\" },"$'\n'
    prev=c$i.out
  done
  if [[ $3 == 1 ]]; then
    lines+='back = { module = "std.unit_delay" }'$'\n'
    wires+="{ from = \"$prev\", to = \"back.in\" },"$'\n'"{ from = \"back.out\", to = \"c1.b\" },"$'\n'
  fi
  write "chain_${1#std.}_$2_$3" "$lines" "$wires"
}

for block in 256 1; do
  b=$(seconds base "$block")
  for count in 100 1000; do
    for loop in 0 1; do
      chain std.add "$count" "$loop"
      t=$(seconds "chain_add_${count}_${loop}" "$block")
      printf 'engine std.add x%-5s loop %s --block %-3s %8s ns per node and frame\n' "$count" "$loop" "$block" \
        "$(per_instance "$t" "$b" "$count")"
    done
  done
  chain std.select 1000 1
  t=$(seconds chain_select_1000_1 "$block")
  printf 'engine std.select x1000 loop 1 --block %-3s %8s ns per node and frame\n' "$block" "$(per_instance "$t" "$b" 1000)"
done
