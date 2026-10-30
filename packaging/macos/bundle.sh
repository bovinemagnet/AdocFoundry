#!/usr/bin/env bash
# Builds AdocFoundry.app and a DMG from a universal binary, with the command-line tools beside
# the app in Contents/MacOS.
#
# usage: packaging/macos/bundle.sh <universal-binary> <version> <output-dir> [<tool>...]
#
# With APPLE_SIGNING_IDENTITY set (a Developer ID Application identity in a keychain), every
# executable is signed with the hardened runtime and a secure timestamp; with APPLE_ID,
# APPLE_TEAM_ID and APPLE_APP_PASSWORD set too, the app and then the DMG are notarised and the
# tickets stapled to both, so Gatekeeper accepts them offline. Without an identity the bundle
# is signed ad hoc, which Apple silicon needs to run it.
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
# The bundled dictionaries' licence, whose notice goes with every copy. Before signing, which
# seals Resources.
mkdir -p "$app/Contents/Resources/licenses"
cp "$here/../../crates/adoc-spell/dictionaries/README.md" "$app/Contents/Resources/licenses/SCOWL.txt"

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

# Submits $1 for notarisation and waits; fails unless Apple accepts it, printing the log.
notarise() {
  local result status id
  result="$(xcrun notarytool submit "$1" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" --wait --output-format json)"
  status="$(python3 -c 'import json, sys; print(json.load(sys.stdin).get("status", ""))' <<< "$result")"
  if [[ "$status" != "Accepted" ]]; then
    id="$(python3 -c 'import json, sys; print(json.load(sys.stdin).get("id", ""))' <<< "$result")"
    echo "notarisation of $1: ${status:-no status}" >&2
    [[ -n "$id" ]] && xcrun notarytool log "$id" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
      --password "$APPLE_APP_PASSWORD" >&2
    return 1
  fi
}

notarising=""
if [[ -n "${APPLE_SIGNING_IDENTITY:-}" && -n "${APPLE_ID:-}" ]]; then
  notarising=1
fi

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  # Inside out, without --deep: each tool, then the app, whose seal covers them.
  for tool in ${tools[@]+"${tools[@]}"}; do
    codesign --force --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" \
      "$app/Contents/MacOS/$(basename "$tool")"
  done
  codesign --force --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$app"
  codesign --verify --strict --deep --verbose=2 "$app"
  if [[ -n "$notarising" ]]; then
    zip="$out/AdocFoundry-notarise.zip"
    ditto -c -k --keepParent "$app" "$zip"
    notarise "$zip"
    rm -f "$zip"
    # Stapled to the app too, so a copy dragged out of the DMG opens offline.
    xcrun stapler staple "$app"
  fi
else
  codesign --force --deep --sign - "$app"
fi

dmg="$out/AdocFoundry-$version-macos-universal.dmg"
rm -f "$dmg"
hdiutil create -volname AdocFoundry -srcfolder "$app" -ov -format UDZO "$dmg"

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$dmg"
  if [[ -n "$notarising" ]]; then
    notarise "$dmg"
    xcrun stapler staple "$dmg"
  fi
fi
echo "$dmg"
