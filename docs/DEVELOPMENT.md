# Development

Developer-facing notes. The end-user installation guide is [INSTALL.md](INSTALL.md) and
deliberately mentions none of this.

## Prerequisites

- **Rust** (stable). <https://rustup.rs>
- **Tauri CLI**: `cargo install tauri-cli --version "^2" --locked`
- **Linux only** — the WebKitGTK stack Tauri renders with:

  ```sh
  sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
      libayatana-appindicator3-dev librsvg2-dev patchelf libxdo-dev libssl-dev
  ```

There is still **no Node/npm step** — no `package.json`, no `node_modules`, nothing to
`npm install`. The frontend is static files served straight from `desktop/frontend`.

One of them is generated. `desktop/assets/css/app.css` is the stylesheet source — the type
scale, both daisyUI themes, the icon plugin — and it compiles to
`desktop/frontend/tailwind.css` with the Tailwind **standalone binary**: a single
executable with a JS runtime inside it, fetched on demand into the gitignored
`tools/bin/`. daisyUI and the icon set are vendored under `desktop/assets/vendor`, so that
binary is the only thing ever downloaded.

```sh
./tools/build-css.sh          # compile; fetches the binary the first time
./tools/build-css.sh --watch  # recompile as you edit
```

**Re-run it after editing `app.css`, `index.html` or `app.js`.** Tailwind only emits the
classes it can see used, and it scans those last two — a class that only exists in a
string built at runtime (`"badge-" + kind`) is invisible to it and renders unstyled with
no error. Write class names out in full.

The compiled file **is committed**, because `cargo build` and `cargo test` do not run
Tauri's `beforeBuildCommand`: if it were generated-only, every plain cargo build would
produce an unstyled app. `build-release.sh` recompiles it minified before bundling, so a
release cannot ship a stale one.

`theme.css` and `styles.css` are not part of that system. They are the printed invoice —
its own paper palette, deliberately not themed — and nothing on screen should use them.

## Running

```sh
cargo test --workspace                    # 86 tests
cargo clippy --workspace --all-targets
cargo run -p realinvoice-desktop          # the app, debug build
```

`cargo tauri dev` also works from `desktop/src-tauri` if you want its file watching.

## Cutting a release

1. Bump the version in **`desktop/src-tauri/tauri.conf.json`** — that is the single
   source of truth. The About pane and the sidebar read it from the running build via
   `app_info`, so nothing else needs editing. Keep the workspace `Cargo.toml` version in
   step so the crate version matches.
2. Run the build:

   ```sh
   ./build-release.sh
   ```

   It prints the full path of every installer it produced.

### Where builds must run

Tauri bundles for the platform it runs on. There is no cross-compilation set up here:

| Installer | Must be built on |
| --- | --- |
| `RealInvoice_<version>_x64-setup.exe` (NSIS) | Windows |
| `RealInvoice_<version>_amd64.AppImage` | Linux |

Building the Windows installer on Linux will not work — NSIS and the MSVC toolchain are
Windows-side. Use a Windows machine, a Windows VM, or a `windows-latest` CI runner.

## Bundle configuration

Everything lives under `bundle` in `desktop/src-tauri/tauri.conf.json`.

- **`targets`** is `["nsis", "appimage"]`, not `"all"` — DEB and RPM need a package
  manager and a terminal, which is exactly what the end user cannot do.
- **`windows.webviewInstallMode`** is `offlineInstaller`. It adds ~127MB to the
  installer and means a till with no internet still installs first time, which is the
  point of an offline-first product. Change it to `{"downloadBootstrapper": {}}` if
  every target machine is reliably online and installer size matters more.
- **`windows.nsis.installMode`** is `perMachine`: installs into Program Files behind one
  standard UAC prompt, with no per-user/per-machine question.
- **`windows.nsis.startMenuFolder`** is deliberately **unset**. Setting it adds a
  "choose a Start Menu folder" page to the wizard; leaving it out skips that page and
  puts the shortcut straight in the Programs list.
- The **desktop shortcut** comes from the finish page's checkbox, which is ticked by
  default in Tauri's stock NSIS template. The **Start Menu entry** and the **Add or
  remove programs** registration are unconditional.

## Icons

`desktop/src-tauri/icons/icon.svg` is the single source. Everything else in that folder
is generated:

```sh
./tools/generate-icons.sh     # needs rsvg-convert, imagemagick, png2icns
```

The generated files are committed, so a release build needs only Rust and the Tauri CLI.

## App identifier

`in.osworks.realinvoice` decides where the app keeps its database:

| OS | Path |
| --- | --- |
| Windows | `%APPDATA%\in.osworks.realinvoice\db.sqlite` |
| Linux | `~/.config/in.osworks.realinvoice/db.sqlite` |
| macOS | `~/Library/Application Support/in.osworks.realinvoice/db.sqlite` |

**Changing it moves that path**, which orphans an existing installation's invoices. It
was settled at `in.osworks.realinvoice` before the first packaged release precisely so it
never has to change again. If it ever must, that is a data migration, not a config edit.
