#!/usr/bin/env bash
# Regenerate every app icon file from assets/app-icon/effectcraft.svg (the canonical vector).
#
# Needs: resvg (brew install resvg / cargo install resvg). On macOS, iconutil also writes the
# .icns. The outputs are committed, so builds and packaging never need these tools.
#
#   packaging/icons.sh
#
# effectcraft.svg and effectcraft-small.svg (the lighter 1024 px trace) come from craftrules
# (assets/app-icons/effectcraft/, which also keeps the source drawing) and are copied in by hand.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
SVG="$DIR/effectcraft.svg"
ID="ai.storyteller.effectcraft"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

command -v resvg >/dev/null || { echo "error: resvg not found (brew install resvg)" >&2; exit 1; }

render() { resvg -w "$2" -h "$2" "$1" "$3" </dev/null; }

# The tile fills its 512 viewBox edge to edge, which suits Windows and Linux. macOS icons keep
# Apple's grid instead: an 824 px body centred on a 1024 canvas with a transparent margin.
MAC="$TMP/mac.svg"
sed -e 's|<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">|<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024"><g transform="translate(100 100) scale(1.609375)">|' \
  -e 's|</svg>[[:space:]]*$|</g></svg>|' "$SVG" >"$MAC"
grep -q 'viewBox="0 0 1024 1024"' "$MAC" || { echo "error: unexpected <svg> header in $SVG" >&2; exit 1; }

render "$SVG" 1024 "$DIR/effectcraft-1024.png"
# Runtime window/Dock icon on macOS (apps/effectcraft/src/main.rs embeds it).
render "$MAC" 512 "$DIR/effectcraft-macos-512.png"

# Linux hicolor theme (the 256 px one is also the runtime icon on Windows and Linux).
for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$DIR/hicolor/${s}x${s}/apps"
  render "$SVG" "$s" "$DIR/hicolor/${s}x${s}/apps/$ID.png"
done
mkdir -p "$DIR/hicolor/scalable/apps"
cp "$DIR/effectcraft-small.svg" "$DIR/hicolor/scalable/apps/$ID.svg"

# Android launcher icons: one mipmap per density, and the APK manifest points at @mipmap/ic_launcher.
# These live in the crate's res/ tree (which aapt reads) rather than in assets/, so their
# attribution sidecars are dot-prefixed: `aapt -S` rejects a second resource name per directory
# and so chokes on `ic_launcher.png.attribution`, but it skips dot files. See `cargo xtask assets`.
for ds in mdpi:48 hdpi:72 xhdpi:96 xxhdpi:144 xxxhdpi:192; do
  d="${ds%%:*}"; s="${ds##*:}"
  mkdir -p "$ROOT/apps/effectcraft-android/res/mipmap-$d"
  render "$SVG" "$s" "$ROOT/apps/effectcraft-android/res/mipmap-$d/ic_launcher.png"
done

# Windows .ico.
ICO_PNGS=()
for s in 16 20 24 32 40 48 64 128 256; do
  render "$SVG" "$s" "$TMP/ico-$s.png"
  ICO_PNGS+=("$TMP/ico-$s.png")
done
(cd "$ROOT" && cargo run -q -p xtask -- ico "$DIR/effectcraft.ico" "${ICO_PNGS[@]}")

# macOS .icns.
if command -v iconutil >/dev/null; then
  SET="$TMP/effectcraft.iconset"
  mkdir -p "$SET"
  for s in 16 32 128 256 512; do
    render "$MAC" "$s" "$SET/icon_${s}x${s}.png"
    render "$MAC" $((s * 2)) "$SET/icon_${s}x${s}@2x.png"
  done
  iconutil -c icns -o "$DIR/effectcraft.icns" "$SET"
else
  echo "warning: iconutil not found (macOS only); effectcraft.icns not regenerated" >&2
fi
echo "icons written to $DIR"
