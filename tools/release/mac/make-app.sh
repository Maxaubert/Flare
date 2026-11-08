#!/bin/bash
# Builds, signs, notarizes and staples Flare.app and its DMG for aarch64-apple-darwin, macOS 15+ (port phase 8,
# regression rules 263 to 266). Everything is written under target/mac-release, never into the repo.
#
#   tools/release/mac/make-app.sh                 ad hoc: Flare.app only (runs on this Mac, cannot be notarized)
#   FLARE_MAC_SIGN_IDENTITY="Developer ID Application: MAX AUBERT (WV7BY8CUM8)" \
#   FLARE_NOTARY_PROFILE=flare-notary tools/release/mac/make-app.sh
#                                                 signed, notarized, stapled Flare.app and Flare-<version>-arm64.dmg
#
# Notary credentials, the first that is set:
#   FLARE_NOTARY_PROFILE                          a notarytool keychain profile (this Mac: flare-notary)
#   FLARE_NOTARY_KEY, FLARE_NOTARY_KEY_ID, FLARE_NOTARY_ISSUER
#                                                 an App Store Connect API key (.p8 path, its id, the issuer id; CI)
#   FLARE_NOTARY_APPLE_ID, FLARE_NOTARY_PASSWORD  an Apple ID and an app-specific password (with the team below)
# FLARE_SIGNING_KEYCHAIN: the keychain that holds the identity (CI: the throwaway one); unset: the search list.
# A Developer ID run first asks `flare-mac-app check-key`, which cannot raise a prompt, whether codesign can use the
# key without one, and stops before building when it cannot (rule 267). Every codesign with the identity goes
# through flare-mac-app (the DMG too, `sign-file`), never straight from this script.
# Velopack (rule 270): the app notarized here is the step-1 artifact. The update package's bundle is signed again
# (`flare-mac-app resign`), notarized and checked after Velopack has added its own files to it, never before.
# Other settings: FLARE_MAC_TEAM (WV7BY8CUM8), FLARE_MAC_OUT (target/mac-release), FLARE_FEATURES (cargo features of
# the flare crate, e.g. live-updates), FLARE_NOTARIZE=0 (sign only), FLARE_SKIP_BUILD=1 (use target/release as it is).
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

mkdir -p "$OUT"
APP="$("$BIN/flare-mac-app" "$OUT" "$BIN" --identity "$IDENTITY" --team "$TEAM")"
echo "built: $APP"
plutil -lint "$APP/Contents/Info.plist" >/dev/null

if [ "$IDENTITY" = - ]; then
  codesign --verify --deep --strict "$APP"
  echo "ad hoc: Flare.app runs on this Mac only; set FLARE_MAC_SIGN_IDENTITY to sign it for release."
  exit 0
fi

notary_auth=()
if [ -n "${FLARE_NOTARY_PROFILE:-}" ]; then
  notary_auth=(--keychain-profile "$FLARE_NOTARY_PROFILE")
elif [ -n "${FLARE_NOTARY_KEY:-}" ]; then
  notary_auth=(--key "$FLARE_NOTARY_KEY" --key-id "${FLARE_NOTARY_KEY_ID:?}" --issuer "${FLARE_NOTARY_ISSUER:?}")
elif [ -n "${FLARE_NOTARY_APPLE_ID:-}" ]; then
  notary_auth=(--apple-id "$FLARE_NOTARY_APPLE_ID" --team-id "$TEAM" --password "${FLARE_NOTARY_PASSWORD:?}")
fi

# Submits one file, waits, fails with Apple's log unless it is Accepted. Prints the submission id.
notarize() {
  local file="$1" json="$OUT/notary-$(basename "$1").json"
  xcrun notarytool submit "$file" "${notary_auth[@]}" --wait --timeout 2h --output-format json >"$json" || true
  local id status
  id="$(plutil -extract id raw -o - "$json" 2>/dev/null || true)"
  status="$(plutil -extract status raw -o - "$json" 2>/dev/null || true)"
  echo "notarization of $(basename "$file"): ${id:-no id}, ${status:-no status}" >&2
  if [ "$status" != Accepted ]; then
    [ -n "$id" ] && xcrun notarytool log "$id" "${notary_auth[@]}" >&2 || true
    return 1
  fi
  echo "$id"
}

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

# The DMG: the stapled app and a link to /Applications (drag to install), signed, notarized and stapled too.
DMG="$OUT/Flare-$VERSION-arm64.dmg"
STAGE="$OUT/dmg-stage"
rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/Flare.app"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "Flare" -srcfolder "$STAGE" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$DMG"
rm -rf "$STAGE"
"$BIN/flare-mac-app" sign-file "$DMG" --identity "$IDENTITY" >/dev/null
notarize "$DMG" >"$OUT/notary-dmg.id"
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG"
spctl -a -t open --context context:primary-signature -vvv "$DMG" 2>&1 | tee "$OUT/spctl-dmg.txt"
grep -q "source=Notarized Developer ID" "$OUT/spctl-dmg.txt"

# The app inside the DMG, attached out of sight (no Finder window, no Desktop icon) and checked there.
MNT="$(mktemp -d "$OUT/mnt.XXXXXX")"
hdiutil attach -quiet -nobrowse -readonly -noautoopen -mountpoint "$MNT" "$DMG"
trap 'hdiutil detach -quiet "$MNT" 2>/dev/null || true; rmdir "$MNT" 2>/dev/null || true' EXIT
codesign --verify --deep --strict "$MNT/Flare.app"
spctl -a -t exec -vvv "$MNT/Flare.app" 2>&1 | grep -q "source=Notarized Developer ID"
[ "$(readlink "$MNT/Applications")" = /Applications ]
hdiutil detach -quiet "$MNT"
rmdir "$MNT"
trap - EXIT

shasum -a 256 "$DMG" "$ZIP" | tee "$OUT/SHA256SUMS"
echo "done: $DMG"
