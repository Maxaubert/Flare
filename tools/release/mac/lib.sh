# Sourced by make-app.sh and pack.sh (port phase 8, rules 1039, 1043, 1046 and 1050): notarization, the DMG, and the
# cleanup on exit. Needs OUT (where notary results go), TEAM, BIN (the folder with flare-mac-app). Prints no key and no
# credential.

# What to undo when the script ends, however it ends: each caller and make_dmg add to one list and the EXIT trap runs
# it newest first, so no one's cleanup replaces another's (rule 1050).
CLEANUP=()
run_cleanup() {
  local i
  for ((i = ${#CLEANUP[@]} - 1; i >= 0; i--)); do
    eval "${CLEANUP[i]}" || true
  done
}
trap run_cleanup EXIT
#   on_exit <command>   (quote paths with printf %q)
on_exit() { CLEANUP+=("$1"); }

# Notary credentials, the first that is set (never a password on notarytool's command line, where `ps` shows it):
#   FLARE_NOTARY_PROFILE                          a notarytool keychain profile (this Mac: flare-notary, made once
#                                                 with `xcrun notarytool store-credentials`; CI: flare-ci, stored in
#                                                 the throwaway keychain FLARE_SIGNING_KEYCHAIN)
#   FLARE_NOTARY_KEY, FLARE_NOTARY_KEY_ID, FLARE_NOTARY_ISSUER
#                                                 an App Store Connect API key (.p8 path, its id, the issuer id)
notary_auth=()
if [ -n "${FLARE_NOTARY_PROFILE:-}" ]; then
  notary_auth=(--keychain-profile "$FLARE_NOTARY_PROFILE")
  [ -n "${FLARE_SIGNING_KEYCHAIN:-}" ] && notary_auth+=(--keychain "$FLARE_SIGNING_KEYCHAIN")
elif [ -n "${FLARE_NOTARY_KEY:-}" ]; then
  notary_auth=(--key "$FLARE_NOTARY_KEY" --key-id "${FLARE_NOTARY_KEY_ID:?}" --issuer "${FLARE_NOTARY_ISSUER:?}")
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

# The DMG of a signed, notarized, stapled app: the app and a link to /Applications (drag to install), signed through
# flare-mac-app (rule 1040), notarized, stapled, then attached out of sight (-nobrowse: no Finder window, no Desktop
# icon) and the app inside checked there.
#   make_dmg <app> <dmg> <identity>
make_dmg() {
  local app="$1" dmg="$2" identity="$3" stage="$OUT/dmg-stage"
  rm -rf "$stage" "$dmg"
  mkdir -p "$stage"
  ditto "$app" "$stage/Flare.app"
  ln -s /Applications "$stage/Applications"
  hdiutil create -quiet -volname "Flare" -srcfolder "$stage" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$dmg"
  rm -rf "$stage"
  "$BIN/flare-mac-app" sign-file "$dmg" --identity "$identity" >/dev/null
  notarize "$dmg" >"$OUT/notary-dmg.id"
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
  spctl -a -t open --context context:primary-signature -vvv "$dmg" 2>&1 | tee "$OUT/spctl-dmg.txt"
  grep -q "source=Notarized Developer ID" "$OUT/spctl-dmg.txt"
  local mnt
  mnt="$(mktemp -d "$OUT/mnt.XXXXXX")"
  on_exit "hdiutil detach -quiet $(printf %q "$mnt") 2>/dev/null; rmdir $(printf %q "$mnt") 2>/dev/null"
  hdiutil attach -quiet -nobrowse -readonly -noautoopen -mountpoint "$mnt" "$dmg"
  codesign --verify --deep --strict "$mnt/Flare.app"
  spctl -a -t exec -vvv "$mnt/Flare.app" 2>&1 | grep -q "source=Notarized Developer ID"
  [ "$(readlink "$mnt/Applications")" = /Applications ]
  hdiutil detach -quiet "$mnt"
  rmdir "$mnt"
}
