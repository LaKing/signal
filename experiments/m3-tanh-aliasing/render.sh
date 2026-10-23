#!/bin/bash
# M3 tanh aliasing experiment: renders the listening package
# (experiments/m3-tanh-aliasing/README.md). Every input is rendered once per
# shaper: a copy of the input project in which every `std.tanh_shaper` is
# replaced by the candidate's type, and nothing else changes. The copies and
# the renders go to renders/m3-tanh-aliasing/ (not tracked); the projects under
# projects/ are only read. Nothing is played through a device.
#
#   experiments/m3-tanh-aliasing/render.sh [--signal PATH]
#
# Needs a release `signal` built with the lab feature:
#   cargo build --release --locked --offline -p signal-cli --features lab
# (default PATH: <cargo target dir>/release/signal). Prints one line per render:
# input, shaper, frames, peak, audio_fnv1a64; fails if a peak exceeds 0.25.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
here="$root/experiments/m3-tanh-aliasing"
out="$root/renders/m3-tanh-aliasing"

signal=""
if [ "${1:-}" = "--signal" ]; then signal=$2; fi
if [ -z "$signal" ]; then
  target=$(cd "$root" && cargo metadata --locked --offline --no-deps --format-version 1 |
    sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
  signal="$target/release/signal"
fi
if ! "$signal" modules 2>/dev/null | grep -q '^lab.tanh_os8_adaa1 '; then
  echo "render.sh: $signal is not a lab build (cargo build --release -p signal-cli --features lab)" >&2
  exit 1
fi

shapers="std.tanh_shaper lab.tanh_adaa2 lab.tanh_os2_adaa1 lab.tanh_os4_adaa1 lab.tanh_os8_adaa1 lab.tanh_os8"

# input name, source folder, seconds, output gain (the same for every shaper;
# empty keeps the project's).
inputs="sweep:$here/inputs/sweep:8:
bass:$here/inputs/bass:8:
kick:$root/projects/kick:8:0.18
studio:$root/projects/studio:16:0.2"

rm -rf "$out"
mkdir -p "$out/projects"
index="$out/index.tsv"
printf 'input\tshaper\tframes\tsample_rate\tpeak\taudio_fnv1a64\n' >"$index"

echo "$inputs" | while IFS=: read -r name source seconds gain; do
  for shaper in $shapers; do
    short=${shaper#*.}
    project="$out/projects/$name-$short"
    cp -R "$source" "$project"
    # Only the type name changes (and the output gain, for the projects
    # whose committed level is above the package's 0.25 peak).
    find "$project" -name '*.toml' -exec sed -i '' "s/\"std\.tanh_shaper\"/\"$shaper\"/g" {} +
    if [ -n "$gain" ]; then
      sed -i '' "s/\(module = \"std.audio_out\", gain = \)[0-9.]*/\1$gain/" "$project/project.toml"
      if ! grep -q "module = \"std.audio_out\", gain = $gain }" "$project/project.toml"; then
        echo "render.sh: $name: could not set the output gain to $gain" >&2
        exit 1
      fi
    fi
    if grep -rq '"std\.tanh_shaper"' "$project" && [ "$shaper" != std.tanh_shaper ]; then
      echo "render.sh: $name: std.tanh_shaper left in the $shaper copy" >&2
      exit 1
    fi
    render="$out/$name/$short"
    mkdir -p "$out/$name"
    "$signal" render "$project" --seconds "$seconds" --out "$render" --json >"$render.json"
    peak=$(sed -n 's/.*"peak": \([0-9.e-]*\).*/\1/p' "$render/metrics.json" | sort -g | tail -1)
    hash=$(sed -n 's/.*"audio_fnv1a64": "\([0-9a-f]*\)".*/\1/p' "$render/manifest.json")
    frames=$(sed -n 's/^  "frames": \([0-9]*\).*/\1/p' "$render/manifest.json")
    rate=$(sed -n 's/^  "sample_rate": \([0-9]*\).*/\1/p' "$render/manifest.json")
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$shaper" "$frames" "$rate" "$peak" "$hash" | tee -a "$index"
    # Fail closed: a missing or unreadable peak is a failure, not a pass.
    if ! awk -v p="$peak" 'BEGIN { exit !(p ~ /^[0-9.eE+-]+$/ && p + 0 <= 0.25) }'; then
      echo "render.sh: $name/$short: peak '$peak' is missing or above 0.25" >&2
      exit 1
    fi
    if [ -z "$hash" ] || [ -z "$frames" ] || [ -z "$rate" ]; then
      echo "render.sh: $name/$short: manifest fields not found" >&2
      exit 1
    fi
  done
done
echo "render.sh: renders in $out (index: $index)"
