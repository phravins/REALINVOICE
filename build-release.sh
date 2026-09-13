#!/usr/bin/env bash
#
# Cuts a release build and tells you exactly where the installers landed.
#
#   ./build-release.sh              # bundle for the host platform
#   ./build-release.sh nsis         # only the Windows installer (must run on Windows)
#   ./build-release.sh appimage     # only the Linux AppImage (must run on Linux)
#
# There is no npm step: the frontend is three static files with no build, so the Tauri
# CLI is driven through cargo and Node is not a dependency of this project at all.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here/desktop/src-tauri"

if ! cargo tauri --version >/dev/null 2>&1; then
  cat >&2 <<'MSG'
The Tauri CLI is not installed. Install it once with:

    cargo install tauri-cli --version "^2" --locked

MSG
  exit 1
fi

version="$(python3 -c 'import json,sys; print(json.load(open("tauri.conf.json"))["version"])')"
echo "==> Building RealInvoice ${version} (release)"
echo

if [ "$#" -gt 0 ]; then
  cargo tauri build --bundles "$1"
else
  cargo tauri build
fi

out="$here/target/release/bundle"
echo
echo "==> Done. Installers:"

found=0
while IFS= read -r artifact; do
  found=1
  size="$(du -h "$artifact" | cut -f1)"
  printf '    %s  (%s)\n' "$artifact" "$size"
done < <(find "$out" -maxdepth 2 -type f \
           \( -name '*-setup.exe' -o -name '*.msi' -o -name '*.AppImage' \
              -o -name '*.dmg' -o -name '*.deb' -o -name '*.rpm' \) 2>/dev/null | sort)

if [ "$found" -eq 0 ]; then
  echo "    none found under $out" >&2
  echo "    (the build may have produced only an unbundled binary)" >&2
  exit 1
fi

echo
echo "    Hand the file above to the end user. Installation steps: docs/INSTALL.md"
