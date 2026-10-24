#!/bin/sh
# Builds Ambry and wraps the binary in target/release/Ambry.app, then opens it.
# A real bundle gets a bundle id, a Dock icon and its own name in the menu bar.
set -eu
cd "$(dirname "$0")/.."
cargo build --release
app=target/release/Ambry.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp target/release/ambry "$app/Contents/MacOS/Ambry"
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
</dict>
</plist>
PLIST
codesign --force --sign - "$app" >/dev/null 2>&1 || true
open "$app"
