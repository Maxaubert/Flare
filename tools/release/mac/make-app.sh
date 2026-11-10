#!/bin/bash
# Builds, signs, notarizes and staples Flare.app and its DMG for aarch64-apple-darwin, macOS 15+ (port phase 8,
# regression rules 1036 to 1039). Everything is written under target/mac-release, never into the repo.
#
#   tools/release/mac/make-app.sh                 ad hoc: Flare.app only (runs on this Mac, cannot be notarized)
#   FLARE_MAC_SIGN_IDENTITY="Developer ID Application: MAX AUBERT (WV7BY8CUM8)" \
#   FLARE_NOTARY_PROFILE=flare-notary tools/release/mac/make-app.sh
#                                                 signed, notarized, stapled Flare.app and Flare-<version>-arm64.dmg
#
# Notary credentials: see lib.sh (a notarytool keychain profile or an App Store Connect API key).
# FLARE_SIGNING_KEYCHAIN: the keychain that holds the identity (CI: the throwaway one); unset: the search list.
# A Developer ID run first asks `flare-mac-app check-key`, which cannot raise a prompt, whether codesign can use the
# key without one, and stops before building when it cannot (rule 1040). Every codesign with the identity goes
# through flare-mac-app (the DMG too, `sign-file`), never straight from this script.
# Velopack (rules 1043 and 1046): a release does not ship this app. pack.sh packs it with vpk, which adds its updater;
# the bundle is signed again by role after that (flare-mac-app as vpk's codesign), notarized and checked inside the
# packages. A release runs this script with FLARE_NOTARIZE=0, then pack.sh (docs/releasing-macos.md).
# Other settings: FLARE_MAC_TEAM (WV7BY8CUM8), FLARE_MAC_OUT (target/mac-release), FLARE_FEATURES (cargo features of
# the flare crate, e.g. live-updates), FLARE_NOTARIZE=0 (sign only), FLARE_SKIP_BUILD=1 (use target/release as it is),
# FLARE_BUILD_ONLY=1 (build the programs and stop: CI, before the signing keychain exists),
# FLARE_BUNDLE_VERSION (the bundle's version instead of the workspace's: the throwaway test packs only).
# Nothing here prints, exports or moves a key or a credential.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
OUT="${FLARE_MAC_OUT:-$ROOT/target/mac-release}"
IDENTITY="${FLARE_MAC_SIGN_IDENTITY:--}"
TEAM="${FLARE_MAC_TEAM:-WV7BY8CUM8}"
NOTARIZE="${FLARE_NOTARIZE:-1}"
BIN="$ROOT/target/release"

[ "$(uname -s)" = Darwin ] || { echo "make-app.sh runs on macOS" >&2; exit 1; }
[ "$(uname -m)" = arm64 ] || { echo "Flare.app is built for Apple Silicon (arm64)" >&2; exit 1; }

VERSION="$(awk '/^\[workspace\.package\]/{w=1;next} /^\[/{w=0} w && /^version *=/{gsub(/.*= *"|".*/,"");print;exit}' "$ROOT/Cargo.toml")"
[ -n "$VERSION" ] || { echo "no workspace version in Cargo.toml" >&2; exit 1; }
echo "Flare $VERSION for macOS (arm64)"

if [ "$IDENTITY" != - ]; then
  # The key check needs flare-mac-app before the rest is built.
  (cd "$ROOT" && cargo build --release --locked -p flare-cef --bin flare-mac-app)
  "$BIN/flare-mac-app" check-key --identity "$IDENTITY"
fi

if [ "${FLARE_SKIP_BUILD:-0}" != 1 ]; then
  features=()
  [ -n "${FLARE_FEATURES:-}" ] && features=(--features "$FLARE_FEATURES")
  (cd "$ROOT" && cargo build --release --locked -p flare ${features[@]+"${features[@]}"})
  (cd "$ROOT" && cargo build --release --locked -p flare-mcp)
  (cd "$ROOT" && cargo build --release --locked -p flare-cef --bin flare-cef-helper --bin flare-mac-app)
fi

# CI builds first and makes the signing keychain only after (rule 1050): the key is in reach for the signing only.
if [ "${FLARE_BUILD_ONLY:-0}" = 1 ]; then
  echo "built the programs (FLARE_BUILD_ONLY=1): no bundle"
  exit 0
fi

mkdir -p "$OUT"
version_arg=()
[ -n "${FLARE_BUNDLE_VERSION:-}" ] && version_arg=(--version "$FLARE_BUNDLE_VERSION")
APP="$("$BIN/flare-mac-app" "$OUT" "$BIN" --identity "$IDENTITY" --team "$TEAM" ${version_arg[@]+"${version_arg[@]}"})"
echo "built: $APP"
plutil -lint "$APP/Contents/Info.plist" >/dev/null

if [ "$IDENTITY" = - ]; then
  codesign --verify --deep --strict "$APP"
  echo "ad hoc: Flare.app runs on this Mac only; set FLARE_MAC_SIGN_IDENTITY to sign it for release."
  exit 0
fi

# shellcheck source=lib.sh
source "$ROOT/tools/release/mac/lib.sh"

if [ "$NOTARIZE" != 1 ] || [ ${#notary_auth[@]} -eq 0 ]; then
  echo "signed, not notarized (no notary credentials or FLARE_NOTARIZE=0)."
  exit 0
fi

# The app: zipped with ditto (keeps the bundle's links and signatures), notarized, the ticket stapled to the app.
ZIP="$OUT/Flare-$VERSION-arm64.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
notarize "$ZIP" >"$OUT/notary-app.id"
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
spctl -a -t exec -vvv "$APP" 2>&1 | tee "$OUT/spctl-app.txt"
grep -q "source=Notarized Developer ID" "$OUT/spctl-app.txt"
# The stapled app, zipped again (what an update download carries).
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"

# The DMG: the stapled app and a link to /Applications, signed, notarized, stapled, checked attached (lib.sh).
DMG="$OUT/Flare-$VERSION-arm64.dmg"
make_dmg "$APP" "$DMG" "$IDENTITY"

shasum -a 256 "$DMG" "$ZIP" | tee "$OUT/SHA256SUMS"
echo "done: $DMG"
