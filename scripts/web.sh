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
cp "$(dirname "$miniquad")/js/gl.js" web/index.html web/gsim.js target/wasm32-unknown-unknown/release/gsim-client.wasm "$out/"
echo "web client in $out"
