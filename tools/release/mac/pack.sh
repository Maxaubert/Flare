#!/bin/bash
# Packs a built Flare.app as a Velopack osx-arm64 release (port phase 8, rules 1043, 1046 and 1048): the update package
# (Flare-<version>-osx-full.nupkg, and a delta when the Releases folder holds the previous full one), the portable zip,
# releases.osx.json; with a Developer ID and notary credentials also the DMG, made from the bundle the packages carry.
#
#   tools/release/mac/pack.sh <Flare.app> <version> <Releases dir> [notes.md]
#
# The bundle vpk zips is signed again by role after vpk has added its updater (UpdateMac) and sq.version: vpk's
# `codesign` is flare-mac-app, through a link named codesign first on vpk's PATH (flare_cef::macos::vpk). vpk then
# notarizes and staples that bundle (FLARE_NOTARY_PROFILE, a notarytool keychain profile: vpk takes no other kind),
# and only then makes the packages. Then the bundle inside each package is checked: unpacked from the .nupkg the way
# Velopack's updater unpacks it, and from the portable zip.
#
# Settings: FLARE_MAC_SIGN_IDENTITY (unset: ad hoc), FLARE_SIGNING_KEYCHAIN, FLARE_NOTARY_PROFILE, FLARE_MAC_TEAM,
# FLARE_MAC_OUT (target/mac-release: the DMG and the checks' scratch), FLARE_MAC_DMG=0 (no DMG), VPK (the vpk command,
# default vpk; the version is VPK_VERSION in the workflows, 1.2.161). Nothing here prints a key or a credential.
set -euo pipefail

[ $# -ge 3 ] || { echo "usage: pack.sh <Flare.app> <version> <Releases dir> [notes.md]" >&2; exit 2; }
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
BIN="$ROOT/target/release"
APP="$(cd "$1" && pwd)"
VERSION="$2"
mkdir -p "$3"
REL="$(cd "$3" && pwd)"
NOTES="${4:-}"
OUT="${FLARE_MAC_OUT:-$ROOT/target/mac-release}"
TEAM="${FLARE_MAC_TEAM:-WV7BY8CUM8}"
BUNDLE_ID=com.flare.terminal
IDENTITY="${FLARE_MAC_SIGN_IDENTITY:--}"
VPK="${VPK:-vpk}"

[ "$(uname -s)" = Darwin ] || { echo "pack.sh runs on macOS" >&2; exit 1; }
[ "$(basename "$APP")" = Flare.app ] || { echo "$APP: the bundle must be Flare.app" >&2; exit 1; }
[ -x "$BIN/flare-mac-app" ] || { echo "no $BIN/flare-mac-app (cargo build --release -p flare-cef --bin flare-mac-app)" >&2; exit 1; }
[ -x "$APP/Contents/MacOS/Flare" ] || { echo "$APP has no Contents/MacOS/Flare" >&2; exit 1; }
mkdir -p "$OUT"
# shellcheck source=lib.sh
source "$ROOT/tools/release/mac/lib.sh"

notarized=0
if [ "$IDENTITY" != - ]; then
  "$BIN/flare-mac-app" check-key --identity "$IDENTITY"
  [ -n "${FLARE_NOTARY_PROFILE:-}" ] && notarized=1
fi

WORK="$(mktemp -d "$OUT/pack.XXXXXX")"
on_exit "rm -rf -- $(printf %q "$WORK")"
mkdir "$WORK/shim"
ln -s "$BIN/flare-mac-app" "$WORK/shim/codesign"

args=(pack --skip-updates --legacyConsole
  --packId Flare --packTitle Flare --packAuthors Max --packVersion "$VERSION"
  --packDir "$APP" --mainExe Flare --runtime osx-arm64 --channel osx --delta BestSpeed
  --noInst --outputDir "$REL"
  --signAppIdentity "$IDENTITY" --signDisableDeep)
[ -n "$NOTES" ] && args+=(--releaseNotes "$NOTES")
[ -n "${FLARE_SIGNING_KEYCHAIN:-}" ] && args+=(--keychain "$FLARE_SIGNING_KEYCHAIN")
[ "$notarized" = 1 ] && args+=(--notaryProfile "$FLARE_NOTARY_PROFILE")
echo "vpk pack Flare $VERSION (osx-arm64), signed by role through flare-mac-app"
FLARE_MAC_SIGN_IDENTITY="$IDENTITY" PATH="$WORK/shim:$PATH" "$VPK" "${args[@]}"

FULL="$REL/Flare-$VERSION-osx-full.nupkg"
PORTABLE="$REL/Flare-osx-Portable.zip"
[ -f "$FULL" ] && [ -f "$PORTABLE" ] && [ -f "$REL/releases.osx.json" ] || { echo "vpk made no $FULL or $PORTABLE" >&2; exit 1; }
grep -q "\"Version\":\"$VERSION\"" "$REL/releases.osx.json" || { echo "releases.osx.json has no $VERSION" >&2; exit 1; }

# The release checks, on a bundle as it comes out of a package (rule 1043).
check_bundle() {
  local app="$1" what="$2"
  codesign --verify --deep --strict "$app"
  [ -x "$app/Contents/MacOS/UpdateMac" ] || { echo "$what: no UpdateMac" >&2; exit 1; }
  grep -q "<version>$VERSION</version>" "$app/Contents/Resources/sq.version" || { echo "$what: sq.version is not $VERSION" >&2; exit 1; }
  [ "$(readlink "$app/Contents/MacOS/sq.version")" = ../Resources/sq.version ] || { echo "$what: no sq.version link" >&2; exit 1; }
  # Each program's role, ad hoc too (rule 1052): the app's identifier and its three keys; the tools (Velopack's
  # UpdateMac, flare-mcp) by their own identifier, under the hardened runtime, with no entitlements.
  local info ents t
  info="$(codesign -d --verbose=2 "$app/Contents/MacOS/Flare" 2>&1)"
  ents="$(codesign -d --entitlements - --xml "$app/Contents/MacOS/Flare" 2>/dev/null)"
  grep -qx "Identifier=$BUNDLE_ID" <<<"$info" || { echo "$what: Flare is not $BUNDLE_ID" >&2; exit 1; }
  for k in device.audio-input device.camera automation.apple-events; do
    grep -q "<key>com.apple.security.$k</key>" <<<"$ents" || { echo "$what: Flare has no $k" >&2; exit 1; }
  done
  [ "$(grep -o '<key>' <<<"$ents" | wc -l | tr -d ' ')" = 3 ] || { echo "$what: Flare has other entitlements" >&2; exit 1; }
  for t in UpdateMac flare-mcp; do
    info="$(codesign -d --verbose=2 "$app/Contents/MacOS/$t" 2>&1)"
    ents="$(codesign -d --entitlements - --xml "$app/Contents/MacOS/$t" 2>/dev/null)"
    grep -qx "Identifier=$BUNDLE_ID.$t" <<<"$info" || { echo "$what: $t is not $BUNDLE_ID.$t" >&2; exit 1; }
    grep -q '^CodeDirectory .*flags=[^ ]*runtime' <<<"$info" || { echo "$what: $t has no hardened runtime" >&2; exit 1; }
    if grep -q '<key>' <<<"$ents"; then echo "$what: $t carries entitlements" >&2; exit 1; fi
  done
  if [ "$IDENTITY" != - ]; then
    "$BIN/flare-mac-app" verify "$app" --team "$TEAM"
  fi
  if [ "$notarized" = 1 ]; then
    xcrun stapler validate "$app"
    spctl -a -t exec -vvv "$app" 2>&1 | grep -q "source=Notarized Developer ID" || { echo "$what: not accepted as notarized" >&2; exit 1; }
  fi
  echo "checked: $what"
}

# The portable zip (ditto keeps the links).
mkdir "$WORK/portable"
ditto -x -k "$PORTABLE" "$WORK/portable"
check_bundle "$WORK/portable/Flare.app" "the portable zip's Flare.app"

# The update package, unpacked as Velopack's updater does (bundle.rs, extract_lib_contents_to_path): every file under
# lib/app/, a `<path>.__symlink` file is a link to the text it holds, every file mode 755.
mkdir "$WORK/nupkg"
unzip -q "$FULL" 'lib/app/*' -d "$WORK/nupkg"
find "$WORK/nupkg/lib/app" -type f -exec chmod 755 {} +
find "$WORK/nupkg/lib/app" -type f -name '*.__symlink' -print0 | while IFS= read -r -d '' f; do
  target="$(cat "$f")"
  rm "$f"
  ln -s "${target%/}" "${f%.__symlink}"
done
mv "$WORK/nupkg/lib/app" "$WORK/nupkg/Flare.app"
check_bundle "$WORK/nupkg/Flare.app" "the update package's Flare.app"

if [ "$notarized" = 1 ] && [ "${FLARE_MAC_DMG:-1}" != 0 ]; then
  DMG="$OUT/Flare-$VERSION-arm64.dmg"
  make_dmg "$WORK/portable/Flare.app" "$DMG" "$IDENTITY"
  echo "dmg: $DMG"
  (cd "$REL" && shasum -a 256 "$(basename "$FULL")" "$(basename "$PORTABLE")") >"$OUT/SHA256SUMS"
  (cd "$OUT" && shasum -a 256 "$(basename "$DMG")") >>"$OUT/SHA256SUMS"
else
  echo "no DMG: it is made only for a signed, notarized pack (FLARE_MAC_SIGN_IDENTITY and FLARE_NOTARY_PROFILE)."
fi
echo "packed: $REL"
