#!/usr/bin/env bash
# Builds the speech engine Flare bundles on macOS (port phase 7, rule 315): whisper-server and parakeet-cli with
# Metal, from whisper.cpp's SOURCE at one pinned commit. The release Windows takes its zip from (b5130) carries no
# macOS program, only an xcframework library, so the Mac engine is built, never downloaded as a binary.
#
# The pin is the commit AND its tree: git checks every object it fetches against its hash, and the tree hash is
# compared after the checkout, so nothing but those exact sources is ever built. Nothing unpinned is fetched: Metal
# is the system's, OpenMP and KleidiAI (both fetched or found outside the tree) are off, and the result links only
# to /usr/lib, /System and its own libraries (checked below, never assumed).
#
# Output, flat in <out>: whisper-server, parakeet-cli, the libraries they link to under the versioned names they
# link by (libggml.0.dylib, libparakeet.1.dylib, ...), each finding the others through an @loader_path rpath,
# the MIT notice, and `.source`: the commit and the SHA-256 of every file, written last. A folder whose `.source`
# names this commit and whose every file still hashes to it is left alone. The build is ad hoc signed by the
# linker; phase 8 signs the bundle with the Developer ID.
#
#   bash tools/build-whisper-macos.sh vendor/whisper [<folder for the test clip and models>]
#
# The second folder, when given, gets the source's own test files: samples/jfk.wav and the tiny Parakeet test
# model (models/for-tests-ggml-parakeet-tdt.bin), for the by-hand and CI checks of a real pass.
set -euo pipefail

# These values MUST say what ENGINE_MACOS in crates/flare-dictation/src/catalog.rs says. The test
# crates/flare-dictation/tests/fetch_whisper.rs reads this file as text and fails when they differ.
REPO='https://github.com/ggml-org/whisper.cpp.git'
TAG='b5130'
COMMIT='927cfce34f31707e17f2bff35c349632fb9e2c3a'
TREE='db312954f67ca741569ac3a889a9ff329e51d952'
PROGRAMS='whisper-server parakeet-cli'

OUT_ARG="${1:?usage: build-whisper-macos.sh <out folder> [<test files folder>]}"
EXTRAS="${2:-}"
HERE="$(cd "$(dirname "$0")" && pwd)"
LICENSE_SRC="$HERE/../LICENSE-whisper.cpp.txt"

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
  echo "whisper: the macOS engine is built on Apple Silicon only" >&2
  exit 2
fi

mkdir -p "$OUT_ARG"
OUT="$(cd "$OUT_ARG" && pwd)"
STAMP="$OUT/.source"

# Every file of the folder must hash to what the stamp says, and the folder may hold nothing the stamp does not
# list (a library planted beside an @loader_path rpath would load): the stamp alone proves nothing (rule 325).
engine_ok() {
  [ -f "$STAMP" ] && [ "$(head -n 1 "$STAMP")" = "$COMMIT" ] || return 1
  (cd "$OUT" && tail -n +2 .source | shasum -a 256 -c --status) || return 1
  listed="$(tail -n +2 "$STAMP" | sed 's/^[0-9a-f]\{64\}  //' | LC_ALL=C sort)"
  present="$(cd "$OUT" && find . ! -type d ! -name .source | sed 's|^\./||' | LC_ALL=C sort)"
  [ "$listed" = "$present" ] || return 1
  for p in $PROGRAMS; do [ -x "$OUT/$p" ] || return 1; done
}
extras_ok() {
  [ -z "$EXTRAS" ] || { [ -f "$EXTRAS/jfk.wav" ] && [ -f "$EXTRAS/for-tests-ggml-parakeet-tdt.bin" ]; }
}
if engine_ok && extras_ok; then
  echo "whisper: $OUT already holds the engine of $TAG ($COMMIT)"
  exit 0
fi
# The folder is emptied before the new engine goes in: never a folder that is not an engine folder already.
if [ -n "$(ls -A "$OUT")" ] && [ ! -f "$STAMP" ]; then
  echo "whisper: $OUT is not empty and holds no engine (.source); refusing to empty it" >&2
  exit 1
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/flare-whisper.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
SRC="$WORK/src"
BUILD="$WORK/build"

echo "whisper: fetching $COMMIT ($TAG)"
git init -q "$SRC"
git -C "$SRC" fetch -q --depth 1 "$REPO" "$COMMIT"
git -C "$SRC" -c advice.detachedHead=false checkout -q FETCH_HEAD
got_commit="$(git -C "$SRC" rev-parse HEAD)"
got_tree="$(git -C "$SRC" rev-parse 'HEAD^{tree}')"
if [ "$got_commit" != "$COMMIT" ] || [ "$got_tree" != "$TREE" ]; then
  echo "whisper: the source is not the pinned one, refusing to build" >&2
  echo "  expected $COMMIT tree $TREE" >&2
  echo "  got      $got_commit tree $got_tree" >&2
  exit 1
fi

echo "whisper: building with Metal"
cmake -S "$SRC" -B "$BUILD" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=ON \
  -DGGML_METAL=ON \
  -DGGML_METAL_EMBED_LIBRARY=ON \
  -DGGML_NATIVE=OFF \
  -DGGML_OPENMP=OFF \
  -DGGML_CPU_KLEIDIAI=OFF \
  -DWHISPER_BUILD_TESTS=OFF \
  -DWHISPER_BUILD_EXAMPLES=ON \
  -DWHISPER_BUILD_SERVER=ON \
  -DWHISPER_CURL=OFF \
  -DWHISPER_SDL2=OFF \
  -DCMAKE_OSX_ARCHITECTURES=arm64 \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=15.0 \
  -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON \
  -DCMAKE_INSTALL_RPATH=@loader_path >"$WORK/configure.log"
# shellcheck disable=SC2086
cmake --build "$BUILD" --config Release -j "$(sysctl -n hw.ncpu)" --target $PROGRAMS >"$WORK/build.log"

BIN="$BUILD/bin"
STAGE="$WORK/stage"
mkdir -p "$STAGE"
for p in $PROGRAMS; do
  cp "$BIN/$p" "$STAGE/$p"
done

# The libraries, by the names the programs link to (@rpath/<name>), followed to their files, flat.
deps_of() { otool -L "$1" | tail -n +2 | awk '{print $1}'; }
changed=1
while [ "$changed" = 1 ]; do
  changed=0
  for f in "$STAGE"/*; do
    for d in $(deps_of "$f"); do
      case "$d" in
        @rpath/*)
          name="${d#@rpath/}"
          if [ ! -f "$STAGE/$name" ]; then
            from="$(find "$BUILD" -name "$name" -print -quit)"
            [ -n "$from" ] || { echo "whisper: $name was not built" >&2; exit 1; }
            cp -L "$from" "$STAGE/$name"
            changed=1
          fi
          ;;
      esac
    done
  done
done

# Nothing outside the system and the folder itself: no Homebrew library, no absolute build path.
for f in "$STAGE"/*; do
  # A library names itself first; that line is its id, not a dependency.
  self="$(otool -D "$f" | tail -n +2 | head -n 1)"
  for d in $(deps_of "$f"); do
    [ "$d" = "$self" ] && continue
    case "$d" in
      /usr/lib/* | /System/Library/*) ;;
      @rpath/*) [ -f "$STAGE/${d#@rpath/}" ] || { echo "whisper: $f needs $d, which is not in the folder" >&2; exit 1; } ;;
      *) echo "whisper: $f links to $d, outside the system and the folder" >&2; exit 1 ;;
    esac
  done
  if otool -l "$f" | grep -A2 LC_RPATH | grep ' path ' | grep -v '@loader_path' >/dev/null; then
    echo "whisper: $f carries an rpath other than @loader_path" >&2
    exit 1
  fi
done

cp "$LICENSE_SRC" "$STAGE/LICENSE-whisper.cpp.txt"

# Into place: the old files out, the new in, the stamp last.
rm -f "$STAMP"
find "$OUT" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
cp -p "$STAGE"/* "$OUT"/
{
  echo "$COMMIT"
  (cd "$OUT" && find . -type f ! -name .source ! -name .source.tmp | sed 's|^\./||' | LC_ALL=C sort | while read -r f; do shasum -a 256 "$f"; done)
} >"$STAMP.tmp"
mv "$STAMP.tmp" "$STAMP"

if [ -n "$EXTRAS" ]; then
  mkdir -p "$EXTRAS"
  cp "$SRC/samples/jfk.wav" "$EXTRAS/jfk.wav"
  cp "$SRC/models/for-tests-ggml-parakeet-tdt.bin" "$EXTRAS/for-tests-ggml-parakeet-tdt.bin"
fi

echo "whisper: $OUT holds the engine of $TAG ($COMMIT): $(ls "$OUT" | tr '\n' ' ')"
