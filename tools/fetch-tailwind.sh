#!/usr/bin/env bash
# Fetches the Tailwind CSS standalone binary into tools/bin, if it is not already there.
#
# Standalone on purpose: it is one file with a JS runtime inside it, so the console keeps
# its "no Node, no node_modules, no npm install" property. daisyUI and the icon set are
# vendored in the repo, so this is the only thing that is ever downloaded.
set -euo pipefail

VERSION="v4.1.14"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/bin"
BIN="$DIR/tailwindcss"

if [ -x "$BIN" ]; then
  echo "Tailwind already present: $("$BIN" --help 2>&1 | head -1)"
  exit 0
fi

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)   ASSET="tailwindcss-linux-x64" ;;
  Linux-aarch64)  ASSET="tailwindcss-linux-arm64" ;;
  Darwin-x86_64)  ASSET="tailwindcss-macos-x64" ;;
  Darwin-arm64)   ASSET="tailwindcss-macos-arm64" ;;
  MINGW*|MSYS*|CYGWIN*) ASSET="tailwindcss-windows-x64.exe" ;;
  *) echo "No Tailwind standalone build for $(uname -s)-$(uname -m)." >&2; exit 1 ;;
esac

mkdir -p "$DIR"
echo "Fetching Tailwind $VERSION ($ASSET)…"
curl -fsSL -o "$BIN" \
  "https://github.com/tailwindlabs/tailwindcss/releases/download/$VERSION/$ASSET"
chmod +x "$BIN"
echo "Installed to $BIN"
