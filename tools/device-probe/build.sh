#!/bin/sh
# Builds the device panel's probe apps for the iOS Simulator, with no Xcode project and no xcodegen: `xcrun swiftc`
# for the simulator's SDK, this folder's Info.plists, and an ad-hoc signature (what Xcode gives a simulator build).
# FPSProbe paints its frame number (the frame bench, `flare-sim-host --bench`); TouchProbe paints the touches and
# keys it saw (the input bench, `flare-sim-host --input-bench`). Tests and the benches only, never shipped. Opens
# nothing.
#
# Usage: tools/device-probe/build.sh [out-dir]   (default: tools/device-probe/build)
# The apps land at <out-dir>/FPSProbe.app and <out-dir>/TouchProbe.app. Needs Xcode with the iOS Simulator SDK
# (DEVELOPER_DIR is honoured).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-"$here/build"}
sdk=$(xcrun --sdk iphonesimulator --show-sdk-path)
ver=$(xcrun --sdk iphonesimulator --show-sdk-version)
arch=$(uname -m)
build() {
  name=$1 src=$2 plist=$3
  app="$out/$name.app"
  rm -rf "$app"
  mkdir -p "$app"
  xcrun --sdk iphonesimulator swiftc -parse-as-library -O \
    -target "$arch-apple-ios$ver-simulator" -sdk "$sdk" \
    -o "$app/$name" "$here/$src"
  cp "$here/$plist" "$app/Info.plist"
  codesign --force --sign - --timestamp=none "$app" >/dev/null
  echo "$app"
}
build FPSProbe FPSProbe-App.swift Info.plist
build TouchProbe TouchProbe-App.swift TouchProbe-Info.plist
