#!/bin/bash
# Stages a build of Flare.app for the installed copy on this Mac (Build ready, macOS port, rule 1132).
#
# Builds Flare.app with make-app.sh into a temp folder, signed with Developer ID (never notarized), then writes the
# stage atomically:
#   <root>/staged.tmp/Flare.app   a copy of the bundle (ditto: links and signature kept)
#   <root>/staged/                staged.tmp renamed into place (the old stage's manifest goes first)
#   <root>/staged/staged.json     written LAST: {commit, built_at, files: {"Contents/MacOS/Flare": sha256, ...}}
# The running app sees the stage, shows "Build ready", and installs it when Max clicks the chip, or when an agent asks
# (`Flare --install-staged`, rule 1326: a Developer ID stage only, once every agent is idle). It NEVER touches
# /Applications or the running bundle: only the app swaps the bundle. A second stage replaces the first.
#
#   tools/release/mac/stage-install.sh                        build and stage for ~/Library/Application Support/Flare
#   FLARE_STAGE_APP=/path/Flare.app tools/release/mac/stage-install.sh   stage a bundle already built (no cargo)
#
# Settings: FLARE_STAGE_ROOT (the root, default ~/Library/Application Support/Flare; the tests use a temp one),
# FLARE_STAGE_APP (a built Flare.app: skips the build), FLARE_STAGE_REPO (the checkout whose commit is named; default
# this one), FLARE_MAC_SIGN_IDENTITY (passed to make-app.sh; unset: the login Keychain's "Developer ID Application"
# identity, team WV7BY8CUM8 first when there are several), FLARE_STAGE_ALLOW_ADHOC=1 (stage an ad hoc bundle anyway).
#
# Rule 1356: only a Developer ID stage. An ad hoc signature is a per-build cdhash, so the installed copy lost its Screen
# Recording grant (TCC) after an ad hoc stage on 2026-11-20; a Developer ID build keeps the grants. With no Developer ID
# identity, or a built or given bundle that is not Developer ID signed, nothing is staged, unless
# FLARE_STAGE_ALLOW_ADHOC=1 (the tests and CI, which have no Keychain identity).
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

ALLOW_ADHOC=0
[ "${FLARE_STAGE_ALLOW_ADHOC:-}" = "1" ] && ALLOW_ADHOC=1
ADHOC_HINT="set FLARE_MAC_SIGN_IDENTITY, or FLARE_STAGE_ALLOW_ADHOC=1 to stage an ad hoc build anyway (it loses the TCC grants)"

# The login Keychain's "Developer ID Application" identity, team WV7BY8CUM8 first when there are several; empty when
# there is none. `find-identity` only reads (no Keychain prompt); its whole answer is read before awk (rule 1342).
find_developer_id() {
  local list
  list="$(security find-identity -v -p codesigning 2>/dev/null || true)"
  printf '%s\n' "$list" | awk -F'"' '
    /"Developer ID Application:/ { if ($2 ~ /\(WV7BY8CUM8\)$/) { pick = $2 } else if (first == "") { first = $2 } }
    END { if (pick != "") print pick; else if (first != "") print first }'
}

# 0 when the bundle is signed with Developer ID: an Authority line for it and a TeamIdentifier that is set. codesign's
# whole answer is read first (rule 1342: a `grep -q` on its pipe failed a signed stage with SIGPIPE, commit 14de4121).
is_developer_id() {
  local info team
  info="$(codesign -dv --verbose=2 "$1" 2>&1 || true)"
  team="$(printf '%s\n' "$info" | sed -n 's/^TeamIdentifier=//p')"
  printf '%s\n' "$info" | grep '^Authority=Developer ID Application:' >/dev/null || return 1
  [ -n "$team" ] && [ "$team" != "not set" ]
}

TMP_BUILD=""
cleanup() { [ -n "$TMP_BUILD" ] && rm -rf "$TMP_BUILD"; true; }
trap cleanup EXIT

if [ -n "${FLARE_STAGE_APP:-}" ]; then
  APP="$FLARE_STAGE_APP"
else
  if [ -z "${FLARE_MAC_SIGN_IDENTITY:-}" ]; then
    FOUND="$(find_developer_id)"
    if [ -n "$FOUND" ]; then
      export FLARE_MAC_SIGN_IDENTITY="$FOUND"
      say "signing with $FOUND (from the login Keychain)"
    elif [ "$ALLOW_ADHOC" = "1" ]; then
      say "no Developer ID identity in the Keychain; building ad hoc (FLARE_STAGE_ALLOW_ADHOC=1)"
    else
      fail "no \"Developer ID Application\" identity in the login Keychain (security find-identity -v -p codesigning), so the build would be ad hoc and lose Flare's TCC grants (Screen Recording); $ADHOC_HINT"
    fi
  fi
  TMP_BUILD="$(mktemp -d "${TMPDIR:-/tmp}/flare-stage.XXXXXX")"
  say "building Flare.app into $TMP_BUILD"
  FLARE_MAC_OUT="$TMP_BUILD" FLARE_NOTARIZE=0 "$REPO/tools/release/mac/make-app.sh"
  APP="$TMP_BUILD/Flare.app"
fi
[ -d "$APP/Contents/MacOS" ] || fail "$APP is not a built Flare.app; build first (tools/release/mac/make-app.sh)"
[ -f "$APP/Contents/MacOS/Flare" ] || fail "$APP has no Contents/MacOS/Flare"

# Checked before anything is written, so a refused bundle leaves the old stage as it was.
SIGNED=0
is_developer_id "$APP" && SIGNED=1
if [ "$SIGNED" = "0" ]; then
  [ "$ALLOW_ADHOC" = "1" ] || fail "$APP is not signed with Developer ID (codesign -dv: no Developer ID Authority or TeamIdentifier); an ad hoc install loses Flare's TCC grants (Screen Recording). Nothing staged; $ADHOC_HINT"
  say "staging a bundle that is not signed with Developer ID (FLARE_STAGE_ALLOW_ADHOC=1)"
fi

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
# Rule 1326: an agent installs only a Developer ID stage (Flare --install-staged); say which this is.
if [ "$SIGNED" = "1" ]; then
  say "the stage is signed with Developer ID: an agent may install it (Flare --install-staged)"
else
  say "the stage is not signed with Developer ID: only a click on Build ready installs it"
fi
