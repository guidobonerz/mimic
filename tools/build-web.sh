#!/usr/bin/env bash
# Build the web frontend: crates/mimic-web as WebAssembly plus its page, into
# target/web (or the folder given). Serve that folder over http, e.g.
#   bash tools/build-web.sh && python -m http.server -d target/web 8080
# and open http://127.0.0.1:8080/. No ROMs or images are bundled.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/target/web}"
cd "$root"
cargo build -p mimic-web --release --target wasm32-unknown-unknown
mkdir -p "$out"
cp target/wasm32-unknown-unknown/release/mimic_web.wasm "$out/mimic.wasm"
cp crates/mimic-web/www/index.html crates/mimic-web/www/mimic.js crates/mimic-web/www/mimic-worker.js "$out/"
cp docs/mimic-user-guide-en.pdf docs/mimic-user-guide-de.pdf "$out/"
if command -v wasm-opt >/dev/null; then
  wasm-opt -O3 "$out/mimic.wasm" -o "$out/mimic.wasm"
fi
echo "web frontend in $out ($(wc -c < "$out/mimic.wasm") bytes of WebAssembly)"
