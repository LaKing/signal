#!/usr/bin/env bash
# Prints Signal's version: MAJOR.MINOR from the workspace version in
# Cargo.toml, and as the last number the commits since the tag
# vMAJOR.MINOR.0 (0 on the tagged commit). One commit, one version: the same
# commit always builds the same version. Without git or the tag it prints
# the workspace version as it is.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BASE=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\([0-9]*\.[0-9]*\)\.[0-9]*"$/\1/p' "$ROOT/Cargo.toml")
[[ -n $BASE ]] || { echo "version: no workspace version in Cargo.toml" >&2; exit 1; }
if COUNT=$(git -C "$ROOT" rev-list --count "v$BASE.0..HEAD" 2>/dev/null); then
  echo "$BASE.$COUNT"
else
  sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml"
fi
