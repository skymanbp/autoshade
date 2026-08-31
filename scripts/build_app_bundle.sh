#!/usr/bin/env bash
#
# Build `AutoShade.app` from binaries already staged in `dist/`.
#
# It is a script rather than a run of workflow YAML for the same reason
# `build_portable.ps1` is: the bundle's payload has to be defined in ONE place,
# and a layout assembled inline in a workflow drifts the first time a runtime
# file moves — silently, because the .app would still open and only a sidecar
# would fail, on someone else's machine.
#
# Inputs (already built and `lipo`-joined by the caller):
#   dist/autoshade        the CLI
#   dist/autoshade-gui    the windowed app
# Output:
#   dist/AutoShade.app
#
# Usage: scripts/build_app_bundle.sh <version>
#
# macOS only: it needs `iconutil`, `codesign` and `sips`, none of which exist
# elsewhere. Refuses rather than producing a bundle that only looks right.

set -euo pipefail

version="${1:-}"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
  echo "usage: $0 <version>   (e.g. 1.2.0)" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: this builds a macOS application bundle and needs iconutil/codesign" >&2
  exit 2
fi

# The version in the bundle's Info.plist is what macOS shows in Finder and what
# a crash report carries, so it must be the version that was actually built —
# not one passed in by hand at the wrong moment. Same check, same reason, as
# build_portable.ps1's.
cargo_version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
if [[ "$cargo_version" != "$version" ]]; then
  echo "error: version mismatch — argument '$version', Cargo.toml '$cargo_version'" >&2
  exit 1
fi

app="dist/AutoShade.app"
macos="$app/Contents/MacOS"
resources="$app/Contents/Resources"

for required in dist/autoshade dist/autoshade-gui LICENSE README.md assets/icon.png; do
  [[ -e "$required" ]] || { echo "error: required file not found: $required" >&2; exit 1; }
done

rm -rf "$app"
mkdir -p "$macos" "$resources"

# BOTH binaries live in Contents/MacOS, and that is deliberate rather than
# tidy-minded. `config::bundled_helper` stops its search at the bundle, so a CLI
# placed anywhere else inside the .app would not find `python/` at all; here,
# both front-ends resolve the same two roots and the same sidecars.
cp dist/autoshade dist/autoshade-gui "$macos/"

# The sidecars, staged exactly like the Windows portable archive: no downloaded
# weights (multi-gigabyte, fetched on first use — and, inside a signed bundle,
# never written here at all; see `config::default_weights_dir`), no bytecode,
# no sidecar tests.
mkdir -p "$resources/python"
/usr/bin/rsync -a \
  --exclude 'weights/' --exclude '__pycache__/' \
  --exclude 'test_*.py' --exclude '*.pyc' \
  python/ "$resources/python/"
cp -R assets "$resources/assets"
cp LICENSE README.md "$resources/"

# --- the icon ---------------------------------------------------------------
# `iconutil` wants an .iconset of exact sizes; `assets/icon.png` is the 1024²
# master every other icon in this repo is cut from.
iconset="$(mktemp -d)/AutoShade.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" assets/icon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" assets/icon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$resources/AutoShade.icns"
rm -rf "$(dirname "$iconset")"

# --- Info.plist -------------------------------------------------------------
# LSMinimumSystemVersion must equal the workflow's MACOSX_DEPLOYMENT_TARGET.
# Claiming support for a system the binary was not built to run on is the one
# way a bundle can fail AFTER Gatekeeper has already let it through.
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>autoshade-gui</string>
	<key>CFBundleIconFile</key>
	<string>AutoShade</string>
	<key>CFBundleIdentifier</key>
	<string>dev.skymanbp-autoshade.autoshade</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>AutoShade</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>${version}</string>
	<key>CFBundleVersion</key>
	<string>${version}</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.photography</string>
	<key>LSMinimumSystemVersion</key>
	<string>12.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
PLIST

plutil -lint "$app/Contents/Info.plist"

# --- signing ----------------------------------------------------------------
# AD-HOC (`--sign -`), and the README says so in those words. Apple silicon
# refuses to execute an unsigned binary at all, so this is what makes the app
# runnable — it is NOT notarisation and does not pretend to be: Gatekeeper will
# still hold a downloaded copy until the user allows it in System Settings.
#
# Deep, and the binaries first: a bundle is signed from the inside out, and
# signing the wrapper over unsigned executables produces a bundle that verifies
# and then dies on launch.
codesign --force --sign - "$macos/autoshade"
codesign --force --sign - "$macos/autoshade-gui"
codesign --force --deep --sign - "$app"
codesign -dv --verbose=4 "$app" 2>&1 | sed 's/^/  /'
codesign --verify --deep --strict --verbose=2 "$app"

# Report what Gatekeeper makes of it. NOT a gate: an ad-hoc signature is
# expected to be rejected here, and the value is the recorded wording, so the
# README's recovery path can be checked against what the OS actually says.
spctl -a -vv "$app" 2>&1 | sed 's/^/  spctl: /' || true

echo "built $app"
