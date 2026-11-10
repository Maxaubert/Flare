#!/bin/bash
# CI only (a GitHub macOS runner): puts the Developer ID certificate and the notary API key where make-app.sh finds
# them, from the repository's secrets, in a throwaway keychain that is deleted when the job ends (rule 1039).
# Secrets (Settings > Secrets and variables > Actions):
#   MACOS_CERT_P12_BASE64     the "Developer ID Application" certificate with its private key, as a .p12, base64
#   MACOS_CERT_PASSWORD       the .p12's password
#   APPLE_API_KEY_P8_BASE64   an App Store Connect API key (.p8, role Developer), base64
#   APPLE_API_KEY_ID          its key id
#   APPLE_API_ISSUER_ID       the issuer id
# Writes FLARE_SIGNING_KEYCHAIN (first), then FLARE_MAC_SIGN_IDENTITY and FLARE_NOTARY_PROFILE to $GITHUB_ENV. The
# profile (flare-ci) is the API key stored by notarytool in the throwaway keychain: vpk pack notarizes with a keychain
# profile only (pack.sh, rule 1048). The .p12 and the .p8 exist on disk only while this script runs: they are removed
# on any exit of it, so no later step (vpk, the cache and artifact actions) can read them (rule 1050).
# Never echoes a secret; the keychain's password is random and lives only in this job.
set -euo pipefail
: "${RUNNER_TEMP:?ci-keychain.sh runs on a GitHub runner}"
: "${MACOS_CERT_P12_BASE64:?}" "${MACOS_CERT_PASSWORD:?}"
: "${APPLE_API_KEY_P8_BASE64:?}" "${APPLE_API_KEY_ID:?}" "${APPLE_API_ISSUER_ID:?}"

KC="$RUNNER_TEMP/flare-signing.keychain-db"
KC_PASS="$(uuidgen)"
P12="$RUNNER_TEMP/flare-cert.p12"
P8="$RUNNER_TEMP/AuthKey_$APPLE_API_KEY_ID.p8"
umask 077
# Both key files go on any exit, a failed import included. The keychain is named in $GITHUB_ENV before it exists,
# so the workflow's cleanup step (if: always()) finds it whatever line fails.
trap 'rm -f "$P12" "$P8"' EXIT
echo "FLARE_SIGNING_KEYCHAIN=$KC" >>"$GITHUB_ENV"
printf '%s' "$MACOS_CERT_P12_BASE64" | base64 --decode >"$P12"
printf '%s' "$APPLE_API_KEY_P8_BASE64" | base64 --decode >"$P8"

security create-keychain -p "$KC_PASS" "$KC"
# Locks itself after the job's own time limit (macos-app.yml timeout-minutes: 150) without use, and on sleep.
security set-keychain-settings -l -u -t 9000 "$KC"
security unlock-keychain -p "$KC_PASS" "$KC"
# Only codesign may use the key, without a prompt (a runner has no one to answer one).
security import "$P12" -k "$KC" -P "$MACOS_CERT_PASSWORD" -f pkcs12 -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KC_PASS" "$KC" >/dev/null
security list-keychains -d user -s "$KC" $(security list-keychains -d user | tr -d '"')
rm -f "$P12"

IDENTITY="$(security find-identity -v -p codesigning "$KC" | awk -F'"' '/Developer ID Application/{print $2; exit}')"
[ -n "$IDENTITY" ] || { echo "no Developer ID Application identity in the certificate secret" >&2; exit 1; }
# The notary profile vpk asks for, in the same throwaway keychain (gone with it at the end of the job). The .p8 is
# not needed after this line.
xcrun notarytool store-credentials flare-ci --key "$P8" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID" \
  --keychain "$KC" >/dev/null
rm -f "$P8"
{
  echo "FLARE_MAC_SIGN_IDENTITY=$IDENTITY"
  echo "FLARE_NOTARY_PROFILE=flare-ci"
} >>"$GITHUB_ENV"
echo "signing keychain ready"
