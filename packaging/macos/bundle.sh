#!/usr/bin/env bash
# Builds AdocFoundry.app and a DMG from a universal binary.
#
# usage: packaging/macos/bundle.sh <universal-binary> <version> <output-dir>
#
# Signs and notarises when APPLE_SIGNING_IDENTITY, APPLE_ID, APPLE_TEAM_ID and
# APPLE_APP_PASSWORD are set; otherwise the bundle is left unsigned.
set -euo pipefail

binary="$1"
version="$2"
out="$3"
here="$(cd "$(dirname "$0")" && pwd)"

app="$out/AdocFoundry.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/adocfoundry"
sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"

# An .icns from the SVG master, at every size macOS asks for.
iconset="$out/AdocFoundry.iconset"
rm -rf "$iconset" && mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  rsvg-convert -w "$size" -h "$size" "$here/../icons/adocfoundry.svg" -o "$iconset/icon_${size}x${size}.png"
  rsvg-convert -w "$((size * 2))" -h "$((size * 2))" "$here/../icons/adocfoundry.svg" -o "$iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AdocFoundry.icns"
rm -rf "$iconset"

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  codesign --force --deep --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$app"
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
