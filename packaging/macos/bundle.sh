#!/usr/bin/env bash
# Builds AdocFoundry.app and a DMG from a universal binary, with the command-line tools beside
# the app in Contents/MacOS.
#
# usage: packaging/macos/bundle.sh <universal-binary> <version> <output-dir> [<tool>...]
#
# Signs and notarises when APPLE_SIGNING_IDENTITY, APPLE_ID, APPLE_TEAM_ID and
# APPLE_APP_PASSWORD are set; otherwise the bundle is signed ad hoc, which Apple silicon needs
# to run it (Developer ID signing and notarisation come in Phase 6).
set -euo pipefail

binary="$1"
version="$2"
out="$3"
shift 3
tools=("$@")
here="$(cd "$(dirname "$0")" && pwd)"

app="$out/AdocFoundry.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/adocfoundry"
# macOS's bash 3.2 treats an empty array as unset under `set -u`.
for tool in ${tools[@]+"${tools[@]}"}; do
  cp "$tool" "$app/Contents/MacOS/"
done
sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"

# An .icns from the 1024-pixel master, at every size macOS asks for.
master="$here/../icons/adocfoundry.png"
iconset="$out/AdocFoundry.iconset"
rm -rf "$iconset" && mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$master" --out "$iconset/icon_${size}x${size}.png" > /dev/null
  sips -z "$((size * 2))" "$((size * 2))" "$master" --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AdocFoundry.icns"
rm -rf "$iconset"

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  codesign --force --deep --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$app"
else
  codesign --force --deep --sign - "$app"
fi

dmg="$out/AdocFoundry-$version-macos-universal.dmg"
rm -f "$dmg"
hdiutil create -volname AdocFoundry -srcfolder "$app" -ov -format UDZO "$dmg"

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" && -n "${APPLE_ID:-}" ]]; then
  codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$dmg"
  xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" --wait
  xcrun stapler staple "$dmg"
fi
echo "$dmg"
