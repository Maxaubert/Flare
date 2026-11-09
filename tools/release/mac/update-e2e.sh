#!/bin/bash
# The Mac update end to end, headless (port phase 8, rule 278): Flare.app packed as the throwaway versions 0.0.1 and
# 0.0.2 (never a real release number), 0.0.1 installed from its portable zip into a temp folder (never
# /Applications), updated to 0.0.2 by the real VelopackFeed and Velopack's UpdateMac with no restart, and the swapped
# bundle checked (crates/flare-update/tests/mac_update.rs). Nothing is launched with `open`, nothing shows a window.
#
#   tools/release/mac/update-e2e.sh                          ad hoc (this Mac only)
#   FLARE_MAC_SIGN_IDENTITY="Developer ID Application: MAX AUBERT (WV7BY8CUM8)" FLARE_NOTARY_PROFILE=flare-notary \
#   tools/release/mac/update-e2e.sh                          signed and notarized (two notarizations, Gatekeeper must
#                                                            accept the bundle before and after the update)
#
# Uses the release programs in target/release as they are (build them first, or FLARE_E2E_BUILD=1). Work folder:
# target/mac-update-e2e (FLARE_MAC_E2E_DIR), emptied first.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
BIN="$ROOT/target/release"
WORK="${FLARE_MAC_E2E_DIR:-$ROOT/target/mac-update-e2e}"
IDENTITY="${FLARE_MAC_SIGN_IDENTITY:--}"

if [ "${FLARE_E2E_BUILD:-0}" = 1 ]; then
  (cd "$ROOT" && cargo build --release --locked -p flare -p flare-mcp)
  (cd "$ROOT" && cargo build --release --locked -p flare-cef --bin flare-cef-helper --bin flare-mac-app)
fi
for b in flare flare-mcp flare-cef-helper flare-mac-app; do
  [ -x "$BIN/$b" ] || { echo "no $BIN/$b (FLARE_E2E_BUILD=1 builds it)" >&2; exit 1; }
done

# The work folder is emptied, so it must be one this script may own (rule 281): an absolute path with no `.` or `..`
# part, resolved through every link, and then a folder target/mac-update-e2e* of this repository or a named folder
# below the temp folder, never target/ or the temp folder itself.
refuse() { echo "$WORK: the work folder must be target/mac-update-e2e* or a folder below the temp folder ($1)" >&2; exit 1; }
case "$WORK" in /*) ;; *) refuse "not absolute" ;; esac
case "/$WORK/" in */../*|*/./*) refuse "a . or .. part" ;; esac
mkdir -p "$WORK"
WORK="$(cd "$WORK" && pwd -P)"
target="$(cd "$ROOT" && pwd -P)/target"
tmp="$(cd "${TMPDIR:-/tmp}" && pwd -P)"
ok=0
case "$WORK" in "$target"/mac-update-e2e*) [ "$(dirname "$WORK")" = "$target" ] && ok=1 ;; esac
for base in "$tmp" /private/tmp; do
  case "$WORK" in "$base"/?*) ok=1 ;; esac
done
[ "$ok" = 1 ] || refuse "resolved to $WORK"
rm -rf -- "$WORK"
mkdir -p "$WORK/Releases"
printf -- '- A throwaway test release\n' >"$WORK/notes.md"

for v in 0.0.1 0.0.2; do
  "$BIN/flare-mac-app" "$WORK/build-$v" "$BIN" --identity "$IDENTITY" --version "$v" >/dev/null
  FLARE_MAC_OUT="$WORK/out-$v" FLARE_MAC_DMG=0 \
    bash "$ROOT/tools/release/mac/pack.sh" "$WORK/build-$v/Flare.app" "$v" "$WORK/Releases" "$WORK/notes.md"
  [ "$v" = 0.0.1 ] && cp "$WORK/Releases/Flare-osx-Portable.zip" "$WORK/Flare-0.0.1-Portable.zip"
done
ls "$WORK/Releases"

expect=0
[ "$IDENTITY" != - ] && [ -n "${FLARE_NOTARY_PROFILE:-}" ] && expect=1
(cd "$ROOT" && FLARE_MAC_RELEASES="$WORK/Releases" FLARE_MAC_INSTALL_ZIP="$WORK/Flare-0.0.1-Portable.zip" \
  FLARE_MAC_EXPECT_NOTARIZED="$expect" \
  cargo test -p flare-update --test mac_update -- --ignored --exact \
  a_packed_flare_app_updates_itself_headless_and_the_swapped_bundle_still_verifies --nocapture)
echo "update e2e passed: $WORK"
