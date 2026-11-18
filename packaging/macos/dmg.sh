#!/bin/sh
# Merges the arm64 + x86_64 release builds into a universal IrChad.app and puts
# that in irchad-macos-universal.dmg
# (drag to Applications). Unsigned beyond ad-hoc: no notarization, so the first
# launch needs right-click → Open. Usage: packaging/macos/dmg.sh
set -eu
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
out=target/dmg
app=$out/IrChad.app
rm -rf "$out"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/irchad" \
  target/aarch64-apple-darwin/release/irchad target/x86_64-apple-darwin/release/irchad
cp LICENSE "$app/Contents/Resources/"
sips -s format icns crates/chad-desktop/assets/icon.png --out "$app/Contents/Resources/irchad.icns" >/dev/null
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>IrChad</string>
  <key>CFBundleDisplayName</key><string>IrChad</string>
  <key>CFBundleIdentifier</key><string>party.gentoo.IrChad</string>
  <key>CFBundleExecutable</key><string>irchad</string>
  <key>CFBundleIconFile</key><string>irchad</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <!-- Without these macOS kills the app when voice/camera asks for access. -->
  <key>NSMicrophoneUsageDescription</key><string>IrChad uses the microphone for voice chat.</string>
  <key>NSCameraUsageDescription</key><string>IrChad uses the camera for video chat.</string>
</dict>
</plist>
EOF
# Ad-hoc: an unsigned bundle downloaded from the web shows as "damaged" instead.
codesign --force --deep -s - "$app"
ln -s /Applications "$out/Applications"
hdiutil create -volname IrChad -srcfolder "$out" -format UDZO -ov irchad-macos-universal.dmg
