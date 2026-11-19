#!/bin/sh
# Builds the device panel's probe app (FPSProbe) for the iOS Simulator, with no Xcode project and no xcodegen:
# `xcrun swiftc` for the simulator's SDK, this folder's Info.plist, and an ad-hoc signature (what Xcode gives a
# simulator build). Tests and the bench only (`flare-sim-host --bench`), never shipped. Opens nothing.
#
# Usage: tools/device-probe/build.sh [out-dir]   (default: tools/device-probe/build)
# The app lands at <out-dir>/FPSProbe.app. Needs Xcode with the iOS Simulator SDK (DEVELOPER_DIR is honoured).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-"$here/build"}
sdk=$(xcrun --sdk iphonesimulator --show-sdk-path)
ver=$(xcrun --sdk iphonesimulator --show-sdk-version)
arch=$(uname -m)
app="$out/FPSProbe.app"
rm -rf "$app"
mkdir -p "$app"
xcrun --sdk iphonesimulator swiftc -parse-as-library -O \
  -target "$arch-apple-ios$ver-simulator" -sdk "$sdk" \
  -o "$app/FPSProbe" "$here/FPSProbe-App.swift"
cp "$here/Info.plist" "$app/Info.plist"
codesign --force --sign - --timestamp=none "$app" >/dev/null
echo "$app"
