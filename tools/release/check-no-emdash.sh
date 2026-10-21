#!/usr/bin/env bash
# The owner's rule: no em-dashes anywhere. -I skips binary files (the screenshots are PNGs).
# One copy, called by ci.yml and release.yml (audit 2026-10-20, H4).
set -u
dash="$(printf '\342\200\224')"
if git grep -I -n "$dash" -- . ':!Cargo.lock'; then
  echo "Em-dash found (owner rule: use en-dashes, commas or rephrase)"
  exit 1
fi