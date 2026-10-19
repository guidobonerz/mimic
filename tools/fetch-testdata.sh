#!/usr/bin/env bash
# Fetch CPU test-suite data into testdata/ (gitignored, never committed).
#
#   tools/fetch-testdata.sh [target-dir]      default: <repo>/testdata
#   export EMU_TESTDATA_DIR=<target-dir>
#
# Sources are pinned to exact commits; the Dormann image is checked by SHA-256.
# Already downloaded files are kept, so re-running is cheap.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${1:-$ROOT/testdata}"

# Tom Harte SingleStepTests 65x02 (MIT), ~850 MB for 6502/v1.
HARTE_COMMIT=2f6980a2d95757486c7bee24355c360e40e2a224
HARTE_URL="https://raw.githubusercontent.com/SingleStepTests/65x02/$HARTE_COMMIT/6502/v1"

# Klaus Dormann 6502 functional test (GPL-3.0 test program; run, never committed).
DORMANN_COMMIT=7954e2dbb49c469ea286070bf46cdd71aeb29e4b
DORMANN_URL="https://raw.githubusercontent.com/Klaus2m5/6502_65C02_functional_tests/$DORMANN_COMMIT/bin_files"
DORMANN_SHA256=fa12bfc761e6f9057e4cc01a665a7b800ff01ae91f598af1e39a1201d01953fd

fetch() { # url file
  if [ ! -s "$2" ]; then
    curl -fsSL --retry 3 -o "$2.part" "$1"
    mv "$2.part" "$2"
  fi
}

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

echo "Dormann functional test -> $DEST/dormann"
mkdir -p "$DEST/dormann"
fetch "$DORMANN_URL/6502_functional_test.bin" "$DEST/dormann/6502_functional_test.bin"
fetch "$DORMANN_URL/6502_functional_test.lst" "$DEST/dormann/6502_functional_test.lst"
actual="$(sha256 "$DEST/dormann/6502_functional_test.bin")"
if [ "$actual" != "$DORMANN_SHA256" ]; then
  echo "error: Dormann image checksum mismatch ($actual)" >&2
  exit 1
fi

echo "Tom Harte 6502 tests -> $DEST/65x02/6502/v1 (256 files)"
mkdir -p "$DEST/65x02/6502/v1"
for i in $(seq 0 255); do
  name="$(printf '%02x.json' "$i")"
  fetch "$HARTE_URL/$name" "$DEST/65x02/6502/v1/$name"
done
echo "$HARTE_COMMIT" > "$DEST/65x02/COMMIT"

echo "done. export EMU_TESTDATA_DIR=$DEST"
