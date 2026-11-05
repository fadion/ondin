#!/usr/bin/env bash
#
# Build Ondin.app around an already-built Ondin binary.
#
#     packaging/macos/build-app.sh <version> <binary> <output-dir>
#
# The bundle is built here rather than by `vpk`, which is handed the finished
# .app: vpk's own Info.plist declares no document type, and its `--plist` is not
# documented as to what it keeps of the generated keys. macOS only: the icon is
# made with `sips` and `iconutil`.
#
set -euo pipefail

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <version> <binary> <output-dir>" >&2
    exit 2
fi

VERSION="${1#v}"
BINARY="$2"
OUTPUT_DIR="$3"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${HERE}/../.." && pwd)"

APP="${OUTPUT_DIR}/Ondin.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BINARY" "$APP/Contents/MacOS/ondin"
chmod 755 "$APP/Contents/MacOS/ondin"

cp "$HERE/Info.plist" "$APP/Contents/Info.plist"
plutil -replace CFBundleShortVersionString -string "$VERSION" "$APP/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$VERSION" "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null

# The whole set up to 512@2x, so a Retina Dock draws artwork rather than an
# upscale: 512@2x consumes the 1024 source whole.
ICONSET="$(mktemp -d)/icon.iconset"
mkdir -p "$ICONSET"
for sz in 16 32 128 256 512; do
    sips -z "$sz" "$sz" "$REPO/icons/icon-1024.png" --out "$ICONSET/icon_${sz}x${sz}.png" >/dev/null
    sips -z $((sz * 2)) $((sz * 2)) "$REPO/icons/icon-1024.png" --out "$ICONSET/icon_${sz}x${sz}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/icon.icns"
rm -rf "$(dirname "$ICONSET")"

# The notices a distributed build must carry, as the other platforms' archives do.
cp "$REPO/LICENSE" "$REPO/THIRD-PARTY-NOTICES.md" "$REPO/README.md" "$APP/Contents/Resources/"
cp "$REPO/crates/ondin-core/assets/fonts/OFL.txt" "$APP/Contents/Resources/Inter-OFL.txt"
cp "$REPO"/licenses/*.txt "$APP/Contents/Resources/"

echo "$APP"
