#!/usr/bin/env bash
# Build release binaries and package them into dist/ for one platform.
# Usage: scripts/ci/package.sh <linux-x86_64 | windows-x86_64 | macos-universal>
set -euo pipefail

name="$1"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
version="${GITHUB_REF_NAME:-dev}"
case "$version" in v[0-9]*) version="${version#v}" ;; *) version="0.0.0" ;; esac
pkg="potato-gsim-$name"
# Only rolling-release builds know their version, so only they update themselves.
if [ "${GITHUB_REF:-}" = "refs/heads/master" ]; then
  export GSIM_BUILD_VERSION="$GITHUB_SHA"
fi
rm -rf dist stage
mkdir -p dist "stage/$pkg"

case "$name" in
  linux-*)
    cargo build --release --locked -p gsim-client -p gsim-server -p gsim-client-core
    cp target/release/gsim-client target/release/gsim-server target/release/gsim-bot README.md "stage/$pkg/"
    tar -czf "dist/$pkg.tar.gz" -C stage "$pkg"
    # Bare client for the self-updater.
    cp target/release/gsim-client "dist/gsim-client-$name"
    ;;

  windows-*)
    cargo build --release --locked -p gsim-client -p gsim-server
    cp target/release/gsim-client.exe target/release/gsim-server.exe README.md "stage/$pkg/"
    (cd stage && 7z a -tzip "../dist/$pkg.zip" "$pkg" >/dev/null)
    cp target/release/gsim-client.exe "dist/gsim-client-$name.exe"
    ;;

  macos-*)
    for target in aarch64-apple-darwin x86_64-apple-darwin; do
      MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --locked -p gsim-client --target "$target"
    done
    app="stage/$pkg/potato-gsim.app"
    mkdir -p "$app/Contents/MacOS"
    lipo -create -output "$app/Contents/MacOS/gsim-client" \
      target/aarch64-apple-darwin/release/gsim-client \
      target/x86_64-apple-darwin/release/gsim-client
    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>potato-gsim</string>
  <key>CFBundleDisplayName</key><string>potato-gsim</string>
  <key>CFBundleIdentifier</key><string>dev.potato.gsim</string>
  <key>CFBundleExecutable</key><string>gsim-client</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
    # Ad-hoc signature: without any signature Apple Silicon refuses to run the binary at
    # all; with it, Gatekeeper shows a warning that the user can override.
    codesign --force --deep --sign - "$app"
    codesign --verify --deep --strict "$app"
    # Whole signed bundle for the self-updater, which swaps it in as `update.app`.
    mkdir -p stage/update
    cp -R "$app" stage/update/update.app
    tar -czf "dist/gsim-client-$name.tar.gz" -C stage/update update.app
    ln -s /Applications "stage/$pkg/Applications"
    # hdiutil occasionally fails with "resource busy" on CI runners.
    for attempt in 1 2 3 4 5; do
      if hdiutil create "dist/$pkg.dmg" -ov -volname "potato-gsim" -fs HFS+ -srcfolder "stage/$pkg"; then
        break
      fi
      [ "$attempt" = 5 ] && exit 1
      sleep 3
    done
    ;;

  *)
    echo "unknown platform '$name'" >&2
    exit 1
    ;;
esac

ls -la dist
