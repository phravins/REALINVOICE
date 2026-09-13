#!/usr/bin/env bash
# Compiles desktop/assets/css/app.css into desktop/frontend/tailwind.css.
#
# The compiled file is committed. That is deliberate: `cargo build` and `cargo test` do
# not run Tauri's beforeBuildCommand, so if the stylesheet were generated-only, every
# plain cargo build would produce an unstyled app. Committing it keeps building the
# console a one-command job for anyone who is not editing styles.
#
# Re-run this after editing app.css, index.html or app.js — the last two because Tailwind
# only emits the classes it can see used.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT/tools/fetch-tailwind.sh"

"$ROOT/tools/bin/tailwindcss" \
  --input "$ROOT/desktop/assets/css/app.css" \
  --output "$ROOT/desktop/frontend/tailwind.css" \
  "$@"
