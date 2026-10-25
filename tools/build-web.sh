#!/usr/bin/env bash
# Build the web frontend: crates/emu-web as WebAssembly plus its page, into
# target/web (or the folder given). Serve that folder over http, e.g.
#   bash tools/build-web.sh && python -m http.server -d target/web 8080
# and open http://127.0.0.1:8080/. No ROMs or images are bundled.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/target/web}"
cd "$root"
cargo build -p emu-web --release --target wasm32-unknown-unknown
mkdir -p "$out"
cp target/wasm32-unknown-unknown/release/emu_web.wasm "$out/mimic.wasm"
cp crates/emu-web/www/index.html crates/emu-web/www/mimic.js "$out/"
if command -v wasm-opt >/dev/null; then
  wasm-opt -O3 "$out/mimic.wasm" -o "$out/mimic.wasm"
fi
echo "web frontend in $out ($(wc -c < "$out/mimic.wasm") bytes of WebAssembly)"
