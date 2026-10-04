#!/bin/sh
# Render the client off-screen (Xvfb + software GL) on a machine without a display and save a
# screenshot. NixOS only. Build first: ./x cargo build --release -p gsim-client
# usage: scripts/screenshot.sh <out.png> <after-seconds> "<xdotool script run after 3 s>" <client args...>
out=$1; after=$2; script=$3; shift 3
p() { nix build --no-link --print-out-paths "nixpkgs#$1^out" 2>/dev/null | head -1; }
MESA=$(p mesa)
export LD_LIBRARY_PATH="$(p libx11)/lib:$(p libxi)/lib:$(p libxkbcommon)/lib:$(p libglvnd)/lib:$MESA/lib:$(p libxcursor)/lib:$(p libxrandr)/lib"
export LIBGL_ALWAYS_SOFTWARE=1 __GLX_VENDOR_LIBRARY_NAME=mesa LIBGL_DRIVERS_PATH=$MESA/lib/dri GALLIUM_DRIVER=llvmpipe
export __EGL_VENDOR_LIBRARY_FILENAMES=$MESA/share/glvnd/egl_vendor.d/50_mesa.json
export HOME=$(dirname "$out")/home; mkdir -p "$HOME"
export GSIM_SHOT_CMD="$(cd "$(dirname "$0")/.." && pwd)/target/release/gsim-client --screenshot $out --screenshot-after $after $*"
export GSIM_SHOT_SCRIPT="$script"
exec nix shell nixpkgs#xvfb-run nixpkgs#xorg-server nixpkgs#xdotool -c xvfb-run -a -s "-screen 0 1400x900x24" \
  sh -c '$GSIM_SHOT_CMD & pid=$!; sleep 3; eval "$GSIM_SHOT_SCRIPT"; wait $pid'
