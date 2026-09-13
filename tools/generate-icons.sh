#!/usr/bin/env bash
#
# Regenerates every app icon from the single source SVG.
#
# Run this only when the brand mark changes; the generated files are committed so a
# release build needs nothing but Rust and the Tauri CLI.
#
#   ./tools/generate-icons.sh
#
# Requires: rsvg-convert (librsvg2-bin), convert (imagemagick), png2icns (icnsutils).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
icons="$here/desktop/src-tauri/icons"
src="$icons/icon.svg"

for tool in rsvg-convert convert png2icns; do
  command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 1; }
done

echo "source: $src"

# Linux and the Tauri dev window use plain PNGs.
for size in 32 128 256 512; do
  rsvg-convert -w "$size" -h "$size" "$src" -o "$icons/${size}x${size}.png"
done
cp "$icons/256x256.png" "$icons/128x128@2x.png"   # Tauri's retina naming
cp "$icons/512x512.png" "$icons/icon.png"

# Windows: one .ico carrying every size the shell asks for, from the taskbar to
# the large-icon view in Explorer.
convert "$icons/512x512.png" -define icon:auto-resize=256,128,64,48,32,24,16 "$icons/icon.ico"

# macOS: .icns from the sizes Apple's iconset expects.
png2icns "$icons/icon.icns" \
  "$icons/512x512.png" "$icons/256x256.png" "$icons/128x128.png" "$icons/32x32.png" \
  >/dev/null

# 512x512.png and 256x256.png are intermediates; the committed set is the rest.
rm -f "$icons/512x512.png" "$icons/256x256.png"

echo "generated:"
ls -la "$icons" | sed 's/^/  /'
