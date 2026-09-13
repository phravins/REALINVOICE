# Office Console — Stage 9 (installers)

Turning the app into something a cashier can install by double-clicking. No business
logic, no UI work — app identity, icons, installer configuration, a one-shot build
script, and two audiences' worth of documentation.

## App identity

| | |
| --- | --- |
| Product name | `RealInvoice` |
| Identifier | `in.osworks.realinvoice` |
| Version | `0.2.0` (single source: `tauri.conf.json`) |
| Publisher | OSWORKS |

**The identifier changed** from `in.osworks.realinvoice.desktop`. That string decides
where the app keeps its database, so changing it moves the data directory — which is
exactly why it had to be settled *now*, before the first packaged release, rather than
after real tills have invoices in them. Nothing has shipped yet, so nobody is affected;
after this, changing it would be a data migration.

## Icons

`desktop/src-tauri/icons/icon.svg` is the single source: the brand mark from the branding
stage, redrawn as a solid indigo tile with a white "R" — solid rather than outlined,
because an outline vanishes at 16px in a taskbar.

`./tools/generate-icons.sh` regenerates everything from it: PNGs at 32/128/256 for Linux,
a seven-size `.ico` (16 → 256) for Windows, and an `.icns` for macOS. The generated files
are committed, so a release build needs only Rust and the Tauri CLI.

## Windows installer (NSIS)

```json
"windows": {
  "webviewInstallMode": { "type": "offlineInstaller", "silent": true },
  "nsis": {
    "installMode": "perMachine",
    "languages": ["English"],
    "displayLanguageSelector": false,
    "installerIcon": "icons/icon.ico",
    "compression": "lzma"
  }
}
```

What the end user sees, and why:

- **Welcome → Licence → Install location → Progress → Finish.** `installMode: perMachine`
  defaults the location to Program Files behind one standard UAC prompt, with no
  per-user/per-machine question.
- **`startMenuFolder` is deliberately unset.** Setting it *adds* a "choose a Start Menu
  folder" page; leaving it out skips that page entirely and puts the shortcut straight in
  the Programs list. One less decision put to a cashier.
- **`displayLanguageSelector: false`** — no language dialog.
- **Desktop shortcut** comes from the finish page's checkbox, which Tauri's stock template
  leaves **ticked by default**; the **Start Menu entry** and the **Add or remove programs**
  registration are unconditional. `INSTALL.md` tells the user to leave both finish-page
  boxes ticked.
- **`webviewInstallMode: offlineInstaller`** embeds the WebView2 runtime. It adds ~127MB
  to the installer, and it means a till with no internet still installs first time — which
  is the entire premise of an offline-first product. A machine that cannot download
  WebView2 mid-install is precisely the machine this software is for.

`targets` is `["nsis", "appimage"]`, not `"all"`: DEB and RPM need a package manager and a
terminal, which is what the end user cannot do.

## Build

```sh
./build-release.sh              # host platform
./build-release.sh appimage     # one target
```

It builds, then prints the full path and size of every installer produced, so there is no
hunting through `target/`. There is **no npm step** — the frontend is static files with no
bundler, so the project has no `package.json` and Node is not a dependency at all.

## Documentation

- **`docs/INSTALL.md`** — for the cashier. Numbered steps, no terminal, no mention of npm,
  cargo, Rust or source. Covers the Windows SmartScreen warning, the UAC prompt, the
  finish-page tick boxes, where the Desktop icon appears, the first-run `admin` password
  shown once on the sign-in screen, and how to uninstall.
- **`docs/DEVELOPMENT.md`** — for us. Toolchain, release process, why each bundle option is
  set the way it is, icon regeneration, and the identifier/data-path warning.

## Verified

Built and ran on Linux:

1. `./build-release.sh appimage` produced a single
   **`RealInvoice_0.2.0_amd64.AppImage`** (74MB) and printed its full path.
2. Ran that file with a **HOME that had never seen the app and a PATH with no cargo,
   rustup or node on it** — the closest thing here to a clean machine. It launched with no
   missing libraries, created `~/.config/in.osworks.realinvoice/db.sqlite`, and showed the
   login screen with the first-run `admin` credentials, exactly as `INSTALL.md` describes.
3. Signed in with that password, attached a seeded customer, added an item and pressed F5.
   **RI-2026-0001, ₹53,100.00** saved and read back from the database in that clean HOME.
   That is the "here's a file → billing an invoice" path, start to finish, with nothing but
   the one downloaded file.
4. The bundled desktop entry is correct: `Name=RealInvoice`, `Categories=Office`,
   `Terminal=false`, with icons at 32/128/256.

### What was *not* verified

**The Windows installer was not built or tested.** Tauri bundles for the platform it runs
on, and NSIS plus the MSVC toolchain are Windows-side; there is no cross-compilation set up
here and this is a Linux container. The NSIS configuration above is written against
Tauri 2.9's bundled installer template, which I read to confirm the page flow, the
shortcut behaviour and the uninstaller registration — but reading a template is not the
same as running an installer.

Producing `RealInvoice_0.2.0_x64-setup.exe` needs a Windows machine, a Windows VM, or a
`windows-latest` CI runner. Until one of those has run `./build-release.sh`, the Windows
half of this stage is configured but unproven.

## Not in this stage

Code signing (unsigned installers trigger the SmartScreen warning `INSTALL.md` walks the
user through), auto-update, a CI release workflow, and macOS bundling — the `.icns` is
generated and wired, but no `.dmg` target is enabled and nothing has been tested on macOS.
