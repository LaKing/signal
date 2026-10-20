#!/bin/bash
#
# Build Signal.app: the GUI (signal-app) as Contents/MacOS/Signal, the CLI
# as Contents/Helpers/signal and the UI library (ui/, without its tests) as
# Contents/Resources/ui, which both serve (ADR 0023), with LICENSE and
# THIRD-PARTY-NOTICES.md in Contents/Resources (ADR 0031), signed ad hoc with the
# hardened runtime and the audio-input entitlement. (Not both binaries in
# MacOS/: on the usual case-insensitive volume Signal and signal are one file.)
#
#   scripts/bundle-app.sh                    debug build into dist/Signal.app
#   scripts/bundle-app.sh --release          release build
#   scripts/bundle-app.sh --target-dir DIR   build in DIR (else CARGO_TARGET_DIR,
#                                            else .cargo/config.toml, as cargo does)
#   scripts/bundle-app.sh --out DIR          write DIR/Signal.app instead of dist/
#
# Builds with --locked --offline (devpanel seats have no network). The
# binaries are taken from the target directory cargo reports, never a fixed
# path. Needs no signing identity. Written for macOS's /bin/bash 3.2.
set -euo pipefail

die () { printf 'bundle-app: %s\n' "$*" >&2; exit 1; }

PROFILE=debug
TARGET_DIR=
OUT=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) PROFILE=release ;;
    --target-dir) [[ $# -ge 2 ]] || die "--target-dir needs a directory"; TARGET_DIR=$2; shift ;;
    --out) [[ $# -ge 2 ]] || die "--out needs a directory"; OUT=$2; shift ;;
    -h|--help) sed -n '3,18p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
  shift
done

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
OUT=${OUT:-$ROOT/dist}

CARGO=${CARGO:-cargo}
if ! command -v "$CARGO" >/dev/null && [[ -x /opt/homebrew/opt/rustup/bin/cargo ]]; then
  CARGO=/opt/homebrew/opt/rustup/bin/cargo
fi
command -v "$CARGO" >/dev/null || die "cargo not found"
RUSTC=$(dirname "$(command -v "$CARGO")")/rustc
[[ -x $RUSTC ]] || RUSTC=rustc
for tool in codesign plutil; do
  command -v "$tool" >/dev/null || die "$tool not found"
done

# --- build ------------------------------------------------------------------
# The bundled helper CLI is built with the `gpu` feature (operator decision,
# M2.7-2 answer A): Signal.app links wgpu anyway, so its helper can list the
# GPUs (`signal devices --compute`). The default CLI build stays wgpu-free.
# The version (scripts/version.sh: MAJOR.MINOR and the commits since its
# tag), built into the binaries through SIGNAL_VERSION.
VERSION=$("$ROOT/scripts/version.sh")
export SIGNAL_VERSION=$VERSION
BUILD=(build --locked --offline -p signal-app -p signal-cli --features signal-cli/gpu)
[[ $PROFILE == release ]] && BUILD+=(--release)
[[ -n $TARGET_DIR ]] && BUILD+=(--target-dir "$TARGET_DIR")
"$CARGO" "${BUILD[@]}"

# The directory cargo used: --target-dir, else what cargo resolves from
# CARGO_TARGET_DIR and its config.
if [[ -z $TARGET_DIR ]]; then
  TARGET_DIR=$("$CARGO" metadata --locked --offline --no-deps --format-version 1 \
    | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
  [[ -n $TARGET_DIR ]] || die "cargo metadata reported no target directory"
fi
BIN=$TARGET_DIR/$PROFILE
for exe in signal-app signal; do
  [[ -x $BIN/$exe ]] || die "$BIN/$exe was not built"
done

# The oldest macOS the toolchain builds for (MACOSX_DEPLOYMENT_TARGET, else
# the target's default).
MINIMUM=$("$RUSTC" --print deployment-target 2>/dev/null | sed -n 's/^MACOSX_DEPLOYMENT_TARGET=//p')
[[ -n $MINIMUM ]] || die "rustc reported no macOS deployment target"

# --- assemble ---------------------------------------------------------------
APP=$OUT/Signal.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Helpers"
cp "$BIN/signal-app" "$APP/Contents/MacOS/Signal"
cp "$BIN/signal" "$APP/Contents/Helpers/signal"
# The UI library the app's window loads from its server
# (signal_ui::library_dir). Its node tests and their entry (ui/index.js) stay
# in the repository.
mkdir -p "$APP/Contents/Resources"
cp -R "$ROOT/ui" "$APP/Contents/Resources/ui"
find "$APP/Contents/Resources/ui" -name '*.test.js' -delete
rm -f "$APP/Contents/Resources/ui/index.js"
[[ -f $APP/Contents/Resources/ui/signal-ui.js ]] || die "the UI library was not copied"
# ADR 0031: the licence and the third-party notice travel with the binaries.
cp "$ROOT/LICENSE" "$ROOT/THIRD-PARTY-NOTICES.md" "$APP/Contents/Resources/"
# printf, not a here-document: macOS's bash 3.2 writes those to a temporary
# file outside $TMPDIR, which a sandbox may refuse.
printf '%s\n' \
  '<?xml version="1.0" encoding="UTF-8"?>' \
  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
  '<plist version="1.0">' \
  '<dict>' \
  '	<key>CFBundleName</key>' \
  '	<string>Signal</string>' \
  '	<key>CFBundleDisplayName</key>' \
  '	<string>Signal</string>' \
  '	<key>CFBundleIdentifier</key>' \
  '	<string>hu.d250.signal</string>' \
  '	<key>CFBundleExecutable</key>' \
  '	<string>Signal</string>' \
  '	<key>CFBundlePackageType</key>' \
  '	<string>APPL</string>' \
  '	<key>CFBundleInfoDictionaryVersion</key>' \
  '	<string>6.0</string>' \
  '	<key>CFBundleShortVersionString</key>' \
  "	<string>$VERSION</string>" \
  '	<key>CFBundleVersion</key>' \
  "	<string>$VERSION</string>" \
  '	<key>LSMinimumSystemVersion</key>' \
  "	<string>$MINIMUM</string>" \
  '	<key>NSMicrophoneUsageDescription</key>' \
  '	<string>Signal uses audio input for metering and analysis.</string>' \
  '	<key>NSHighResolutionCapable</key>' \
  '	<true/>' \
  '	<key>NSAppTransportSecurity</key>' \
  '	<dict>' \
  '		<key>NSAllowsLocalNetworking</key>' \
  '		<true/>' \
  '	</dict>' \
  '	<key>CFBundleDocumentTypes</key>' \
  '	<array>' \
  '		<dict>' \
  '			<key>CFBundleTypeName</key>' \
  '			<string>Signal Project</string>' \
  '			<key>CFBundleTypeRole</key>' \
  '			<string>Editor</string>' \
  '			<key>LSHandlerRank</key>' \
  '			<string>Owner</string>' \
  '			<key>LSItemContentTypes</key>' \
  '			<array>' \
  '				<string>hu.d250.signal.project</string>' \
  '			</array>' \
  '		</dict>' \
  '	</array>' \
  '	<key>UTExportedTypeDeclarations</key>' \
  '	<array>' \
  '		<dict>' \
  '			<key>UTTypeIdentifier</key>' \
  '			<string>hu.d250.signal.project</string>' \
  '			<key>UTTypeDescription</key>' \
  '			<string>Signal Project</string>' \
  '			<key>UTTypeConformsTo</key>' \
  '			<array>' \
  '				<string>public.data</string>' \
  '				<string>public.content</string>' \
  '			</array>' \
  '			<key>UTTypeTagSpecification</key>' \
  '			<dict>' \
  '				<key>public.filename-extension</key>' \
  '				<array>' \
  '					<string>signal</string>' \
  '				</array>' \
  '			</dict>' \
  '		</dict>' \
  '	</array>' \
  '</dict>' \
  '</plist>' \
  > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"

# --- sign -------------------------------------------------------------------
# Ad hoc, hardened runtime. The CLI first (it plays and lists devices too),
# then the bundle, whose seal covers it.
ENTITLEMENTS=$ROOT/scripts/Signal.entitlements
codesign --force --sign - --options runtime --entitlements "$ENTITLEMENTS" \
  --identifier hu.d250.signal.cli "$APP/Contents/Helpers/signal"
codesign --force --sign - --options runtime --entitlements "$ENTITLEMENTS" "$APP"
codesign --verify --deep --strict "$APP"

echo "bundle-app: $APP ($PROFILE, version $VERSION, macOS $MINIMUM+, from $BIN)"
