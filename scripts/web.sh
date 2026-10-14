#!/bin/sh
# Build the client for browsers and put the page together in target/web
# (a server hands it out with --web-dir, or GSIM_WEB_DIR).
set -e
cd "$(dirname "$0")/.."
cargo build --release --locked -p gsim-client --target wasm32-unknown-unknown
out=target/web
mkdir -p "$out"
# The window library's own script, of the version this build uses.
miniquad=$(cargo metadata --locked --format-version 1 | tr ',' '\n' | grep -o '"manifest_path":"[^"]*/miniquad-[0-9][^"]*' | head -n 1 | cut -d'"' -f4)
cp web/index.html web/gsim.js web/worker.js target/wasm32-unknown-unknown/release/gsim-client.wasm "$out/"
# Its script gives the slash key the wrong number (189 instead of 47): the game never sees it,
# and Firefox opens its quick find instead. Mended in the copy.
sed 's/case "Slash": return 189;/case "Slash": return 47;/' "$(dirname "$miniquad")/js/gl.js" > "$out/gl.js"
grep -q 'case "Slash": return 47;' "$out/gl.js" || { echo "gl.js: the slash key's number was not mended" >&2; exit 1; }
echo "web client in $out"
