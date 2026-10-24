#!/bin/bash
#
# Build a release (ADR 0031 §4): the same script locally and on GitHub Actions.
#
#   scripts/release.sh                    into dist/release-<version>/
#   scripts/release.sh --out DIR          into DIR
#   scripts/release.sh --target-dir DIR   build in DIR (else CARGO_TARGET_DIR,
#                                         else .cargo/config.toml, as cargo does)
#
# Writes, named with scripts/version.sh's <v>:
#   Signal-<v>-macos-arm64.zip     Signal.app (scripts/bundle-app.sh --release,
#                                  with LICENSE and THIRD-PARTY-NOTICES.md in
#                                  Contents/Resources), by ditto -c -k --norsrc --noextattr --keepParent
#   signal-<v>-macos-arm64.tar.gz  the `signal` CLI, LICENSE, THIRD-PARTY-NOTICES.md
#                                  and a short README
#   Signal-<v>-projects.zip        projects/, without git-ignored files
#   SHA256SUMS                     of the three archives
#   RELEASE-NOTES.md               CHANGELOG.md's section for <v> (else for
#                                  MAJOR.MINOR.0) and how to install
#
# Checks THIRD-PARTY-NOTICES.md first (node scripts/third-party-notices.mjs
# --check). Builds offline (--locked --offline), so run ./install.sh, or
# `cargo fetch --locked` in CI, first. The app is built in a temporary
# directory: dist/Signal.app and projects/ are only read. Needs git (the
# version and the projects' file list). Written for macOS's /bin/bash 3.2.
set -euo pipefail

die () { printf 'release: %s\n' "$*" >&2; exit 1; }

OUT=
TARGET_DIR=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) [[ $# -ge 2 ]] || die "--out needs a directory"; OUT=$2; shift ;;
    --target-dir) [[ $# -ge 2 ]] || die "--target-dir needs a directory"; TARGET_DIR=$2; shift ;;
    -h|--help) sed -n '3,25p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
  shift
done

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
for tool in git node ditto shasum tar plutil; do
  command -v "$tool" >/dev/null || die "$tool not found"
done
git rev-parse --git-dir >/dev/null 2>&1 || die "not a git checkout"

VERSION=$(scripts/version.sh)
OUT=${OUT:-$ROOT/dist/release-$VERSION}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
STAGE=$(mktemp -d "${TMPDIR:-/tmp}/signal-release.XXXXXX")
trap 'rm -rf "$STAGE"' EXIT

APP_ZIP=Signal-$VERSION-macos-arm64.zip
CLI_NAME=signal-$VERSION-macos-arm64
CLI_TAR=$CLI_NAME.tar.gz
PROJECTS_ZIP=Signal-$VERSION-projects.zip

# --- the notice, then the build ---------------------------------------------
node scripts/third-party-notices.mjs --check
# Releases are signed ad hoc (ADR 0031), whatever SIGNAL_SIGN_IDENTITY says.
BUNDLE=(--release --out "$STAGE" --sign -)
[[ -n $TARGET_DIR ]] && BUNDLE+=(--target-dir "$TARGET_DIR")
scripts/bundle-app.sh "${BUNDLE[@]}"
APP=$STAGE/Signal.app
MINIMUM=$(plutil -extract LSMinimumSystemVersion raw -o - "$APP/Contents/Info.plist")

# Archives carry no AppleDouble (._*) files: ditto ignores COPYFILE_DISABLE,
# so each ditto call says --norsrc --noextattr (the app's signature does not
# live in extended attributes).
export COPYFILE_DISABLE=1

# --- Signal.app ---------------------------------------------------------------
rm -f "$OUT/$APP_ZIP"
ditto -c -k --norsrc --noextattr --keepParent "$APP" "$OUT/$APP_ZIP"

# --- the CLI ------------------------------------------------------------------
# The app's helper: the same signed binary, built with the `gpu` feature.
mkdir "$STAGE/$CLI_NAME"
cp "$APP/Contents/Helpers/signal" "$STAGE/$CLI_NAME/signal"
cp LICENSE THIRD-PARTY-NOTICES.md "$STAGE/$CLI_NAME/"
# printf, not a here-document: macOS's bash 3.2 writes those to a temporary
# file outside $TMPDIR, which a sandbox may refuse.
printf '%s\n' \
  "# signal $VERSION (macOS, Apple Silicon)" \
  '' \
  "The \`signal\` command line of Signal $VERSION: render, replay, play, pack and inspect" \
  "Signal projects. Needs an Apple Silicon Mac with macOS $MINIMUM or later." \
  '' \
  '    xattr -d com.apple.quarantine signal     # once: the binary is signed ad hoc, not notarized' \
  '    ./signal --version' \
  '    ./signal render projects/kick --seconds 10 --out kick-render   # offline, no audio device' \
  '    ./signal play projects/kick                                     # real time; Enter stops' \
  '    ./signal --help' \
  '' \
  "The example projects are in Signal-$VERSION-projects.zip. \`signal ui\` serves the web UI" \
  'library that Signal.app carries: run it as Signal.app/Contents/Helpers/signal.' \
  '' \
  'Signal is free software under the GNU General Public License, version 3 or later' \
  '(LICENSE). The crates it links and their licences: THIRD-PARTY-NOTICES.md.' \
  > "$STAGE/$CLI_NAME/README.md"
rm -f "$OUT/$CLI_TAR"
tar -czf "$OUT/$CLI_TAR" -C "$STAGE" "$CLI_NAME"

# --- the example projects -----------------------------------------------------
# Tracked and untracked files, without what .gitignore excludes (renders, …).
git ls-files -z --cached --others --exclude-standard -- projects \
  | tar --null -T - -cf - | tar -xf - -C "$STAGE"
[[ -f $STAGE/projects/kick/project.toml ]] || die "projects/ was not copied"
rm -f "$OUT/$PROJECTS_ZIP"
ditto -c -k --norsrc --noextattr --keepParent "$STAGE/projects" "$OUT/$PROJECTS_ZIP"

# --- checksums ----------------------------------------------------------------
(cd "$OUT" && shasum -a 256 "$APP_ZIP" "$CLI_TAR" "$PROJECTS_ZIP" > SHA256SUMS)

# --- release notes ------------------------------------------------------------
# The body of CHANGELOG.md's `## [<v>]` section, else `## [MAJOR.MINOR.0]`.
section () {
  awk -v head="## [$1]" '
    index($0, head) == 1 { on = 1; next }
    on && /^## / { exit }
    on { print }
  ' CHANGELOG.md
}
NOTES=$(section "$VERSION")
[[ -n $NOTES ]] || NOTES=$(section "${VERSION%.*}.0")
[[ -n $NOTES ]] || die "CHANGELOG.md has no section for $VERSION or ${VERSION%.*}.0"
{
  printf '%s\n' "$NOTES" | sed -e '/./,$!d'
  printf '%s\n' \
    '' \
    '## Install' \
    '' \
    "Signal $VERSION runs on a Mac with **Apple Silicon** (M1 or later) and **macOS $MINIMUM or later**." \
    '' \
    "1. Download \`$APP_ZIP\` and open it: the Finder unzips \`Signal.app\`. Move it to Applications if you like." \
    '2. Signal is signed ad hoc, not notarized, so macOS refuses it on a plain double-click the first time.' \
    '   Double-click it once and dismiss the warning, then open **System Settings → Privacy & Security**,' \
    '   click **Open Anyway** next to the note about Signal, and confirm. (On macOS 14 and earlier,' \
    '   **right-click** `Signal.app` → **Open** does the same.) Or, in Terminal:' \
    '   `xattr -dr com.apple.quarantine Signal.app`. Later starts need nothing extra.' \
    '   macOS asks once for microphone access, which Signal uses for input meters.' \
    "3. Open an example project: download \`$PROJECTS_ZIP\` and unzip it, then in Signal choose" \
    '   **File → Open…** and pick a folder such as `projects/kick` (every folder with a `project.toml` is a project).' \
    '' \
    "The command line alone is \`$CLI_TAR\` (\`signal\`, with a README); Signal.app carries the same" \
    'binary as `Signal.app/Contents/Helpers/signal`.' \
    '' \
    'Check the downloads with `shasum -a 256 -c SHA256SUMS`.' \
    '' \
    'Signal is free software under the GNU General Public License, version 3 or later (`LICENSE`); the' \
    'crates it links and their licences are listed in `THIRD-PARTY-NOTICES.md`, inside both downloads.'
} > "$OUT/RELEASE-NOTES.md"

# --- report -------------------------------------------------------------------
echo "release: $VERSION (macOS $MINIMUM+) in $OUT"
for asset in "$APP_ZIP" "$CLI_TAR" "$PROJECTS_ZIP" SHA256SUMS RELEASE-NOTES.md; do
  printf '  %-40s %12s bytes\n' "$asset" "$(wc -c < "$OUT/$asset" | tr -d ' ')"
done
cat "$OUT/SHA256SUMS"
