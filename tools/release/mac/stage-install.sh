#!/bin/bash
# Stages a build of Flare.app for the installed copy on this Mac (Build ready, macOS port, rule 1132).
#
# Builds Flare.app with make-app.sh into a temp folder (ad hoc, or FLARE_MAC_SIGN_IDENTITY when set; never
# notarized), then writes the stage atomically:
#   <root>/staged.tmp/Flare.app   a copy of the bundle (ditto: links and signature kept)
#   <root>/staged/                staged.tmp renamed into place (the old stage's manifest goes first)
#   <root>/staged/staged.json     written LAST: {commit, built_at, files: {"Contents/MacOS/Flare": sha256, ...}}
# The running app sees the stage, shows "Build ready", and installs it when Max clicks the chip. It NEVER touches
# /Applications or the running bundle: only the click in the app swaps the bundle. A second stage replaces the first.
#
#   tools/release/mac/stage-install.sh                        build and stage for ~/Library/Application Support/Flare
#   FLARE_STAGE_APP=/path/Flare.app tools/release/mac/stage-install.sh   stage a bundle already built (no cargo)
#
# Settings: FLARE_STAGE_ROOT (the root, default ~/Library/Application Support/Flare; the tests use a temp one),
# FLARE_STAGE_APP (a built Flare.app: skips the build), FLARE_STAGE_REPO (the checkout whose commit is named; default
# this one), FLARE_MAC_SIGN_IDENTITY (passed to make-app.sh; unset: ad hoc).
set -euo pipefail

REPO="${FLARE_STAGE_REPO:-$(cd "$(dirname "$0")/../../.." && pwd)}"
ROOT="${FLARE_STAGE_ROOT:-$HOME/Library/Application Support/Flare}"
say() { echo "$(date +%H:%M:%S) $*"; }
fail() { say "FAILED: $*" >&2; exit 1; }

# The stage never goes into an Applications folder or a bundle: that is the installed copy's place.
case "$ROOT" in
  /Applications | /Applications/* | */Applications | */Applications/* | *.app | *.app/*)
    fail "$ROOT is not a stage root (it is in an Applications folder or a bundle)" ;;
esac

TMP_BUILD=""
cleanup() { [ -n "$TMP_BUILD" ] && rm -rf "$TMP_BUILD"; true; }
trap cleanup EXIT

if [ -n "${FLARE_STAGE_APP:-}" ]; then
  APP="$FLARE_STAGE_APP"
else
  TMP_BUILD="$(mktemp -d "${TMPDIR:-/tmp}/flare-stage.XXXXXX")"
  say "building Flare.app into $TMP_BUILD"
  FLARE_MAC_OUT="$TMP_BUILD" FLARE_NOTARIZE=0 "$REPO/tools/release/mac/make-app.sh"
  APP="$TMP_BUILD/Flare.app"
fi
[ -d "$APP/Contents/MacOS" ] || fail "$APP is not a built Flare.app; build first (tools/release/mac/make-app.sh)"
[ -f "$APP/Contents/MacOS/Flare" ] || fail "$APP has no Contents/MacOS/Flare"

COMMIT="$(git -C "$REPO" rev-parse HEAD 2>/dev/null)" || fail "cannot read the commit of $REPO"
[ -n "$(git -C "$REPO" status --porcelain --untracked-files=no 2>/dev/null)" ] && COMMIT="$COMMIT-dirty"
BUILT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

mkdir -p "$ROOT"
STAGED="$ROOT/staged"
TMP="$ROOT/staged.tmp"
rm -rf "$TMP"
mkdir -p "$TMP"
ditto "$APP" "$TMP/Flare.app"

# JSON string escape for a file name (backslash and quote; bundle names hold nothing else that needs it).
esc() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'; }
FILES=""
COUNT=0
while IFS= read -r -d '' f; do
  rel="${f#"$TMP/Flare.app/"}"
  hash="$(shasum -a 256 "$f" | awk '{print $1}')"
  FILES="$FILES${FILES:+,}
    \"$(esc "$rel")\": \"$hash\""
  COUNT=$((COUNT + 1))
done < <(find "$TMP/Flare.app" -type f -print0 | sort -z)

# The old stage goes, its manifest first, so a half-replaced stage is never ready; then the new one is renamed in.
rm -f "$STAGED/staged.json"
rm -rf "$STAGED"
mv "$TMP" "$STAGED"

# The manifest last, written next to itself and renamed: Flare never reads half of it.
printf '{\n  "commit": "%s",\n  "built_at": "%s",\n  "files": {%s\n  }\n}\n' "$COMMIT" "$BUILT" "$FILES" \
  >"$STAGED/staged.json.tmp"
mv "$STAGED/staged.json.tmp" "$STAGED/staged.json"
say "staged ${COMMIT:0:7} for $ROOT ($COUNT files); click Build ready in Flare to install it"
