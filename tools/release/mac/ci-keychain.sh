#!/bin/bash
# CI only (a GitHub macOS runner): puts the Developer ID certificate and the notary API key where make-app.sh finds
# them, from the repository's secrets, in a throwaway keychain that is deleted when the job ends (rule 266).
# Secrets (Settings > Secrets and variables > Actions):
#   MACOS_CERT_P12_BASE64     the "Developer ID Application" certificate with its private key, as a .p12, base64
#   MACOS_CERT_PASSWORD       the .p12's password
#   APPLE_API_KEY_P8_BASE64   an App Store Connect API key (.p8, role Developer), base64
#   APPLE_API_KEY_ID          its key id
#   APPLE_API_ISSUER_ID       the issuer id
# Writes FLARE_SIGNING_KEYCHAIN and FLARE_NOTARY_KEY (first), then FLARE_MAC_SIGN_IDENTITY, FLARE_NOTARY_KEY_ID and
# FLARE_NOTARY_ISSUER to $GITHUB_ENV.
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
# The .p12 goes on any exit, a failed import included; the keychain and the .p8 are named in $GITHUB_ENV before they
# exist, so the workflow's cleanup step (if: always()) finds them whatever line fails.
trap 'rm -f "$P12"' EXIT
{
  echo "FLARE_SIGNING_KEYCHAIN=$KC"
  echo "FLARE_NOTARY_KEY=$P8"
} >>"$GITHUB_ENV"
printf '%s' "$MACOS_CERT_P12_BASE64" | base64 --decode >"$P12"
printf '%s' "$APPLE_API_KEY_P8_BASE64" | base64 --decode >"$P8"

security create-keychain -p "$KC_PASS" "$KC"
security set-keychain-settings -lut 21600 "$KC"
security unlock-keychain -p "$KC_PASS" "$KC"
# Only codesign may use the key, without a prompt (a runner has no one to answer one).
security import "$P12" -k "$KC" -P "$MACOS_CERT_PASSWORD" -f pkcs12 -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KC_PASS" "$KC" >/dev/null
security list-keychains -d user -s "$KC" $(security list-keychains -d user | tr -d '"')
rm -f "$P12"

IDENTITY="$(security find-identity -v -p codesigning "$KC" | awk -F'"' '/Developer ID Application/{print $2; exit}')"
[ -n "$IDENTITY" ] || { echo "no Developer ID Application identity in the certificate secret" >&2; exit 1; }
{
  echo "FLARE_MAC_SIGN_IDENTITY=$IDENTITY"
  echo "FLARE_NOTARY_KEY_ID=$APPLE_API_KEY_ID"
  echo "FLARE_NOTARY_ISSUER=$APPLE_API_ISSUER_ID"
} >>"$GITHUB_ENV"
echo "signing keychain ready"
