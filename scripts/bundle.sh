#!/bin/sh
# Builds Ambry and wraps the binary in target/release/Ambry.app, then opens it.
# A real bundle gets a bundle id, a Dock icon and its own name in the menu bar.
# --classic shows the app without the macOS 26 design (Liquid Glass), as on earlier systems:
# the binary is marked as built with the macOS 15 SDK, since macOS 27 no longer honours
# UIDesignRequiresCompatibility for apps built with its own SDK. Other arguments go to the app.
set -eu
cd "$(dirname "$0")/.."
compatibility=""
classic=""
for arg in "$@"; do
  shift
  if [ "$arg" = "--classic" ]; then
    classic=1
    compatibility="<key>UIDesignRequiresCompatibility</key><true/>"
  else
    set -- "$@" "$arg"
  fi
done
cargo build --release
app=target/release/Ambry.app
pkill -x Ambry 2>/dev/null && sleep 1 || true
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
if [ -n "$classic" ]; then
  xcrun vtool -set-build-version macos 11.0 15.5 -replace \
    -output "$app/Contents/MacOS/Ambry" target/release/ambry
else
  cp target/release/ambry "$app/Contents/MacOS/Ambry"
fi
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Ambry</string>
  <key>CFBundleDisplayName</key><string>Ambry</string>
  <key>CFBundleIdentifier</key><string>dev.ambry.Ambry</string>
  <key>CFBundleExecutable</key><string>Ambry</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  $compatibility
</dict>
</plist>
PLIST
codesign --force --sign - "$app" >/dev/null 2>&1 || true
open "$app" --args "$@"
