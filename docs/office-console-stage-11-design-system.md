# Office Console — Stage 11 (the shared design system)

The console's look was hand-built over stages 6-8: a token file, two themes, a flat
layout. It worked, but it was *this app's* look, invented here. The Back-Office Web
(QuantumBilling) has its own — Tailwind v4, daisyUI v5, Inter, a retuned type scale — and
two runtimes of one product drifting apart visually is the thing "one core, three
runtimes" exists to prevent.

This stage adopts the web app's system on the desktop, close to verbatim. It is the
toolchain and the shell; the panes convert after it, one at a time.

## What was adopted, and what could not be

| From the Back-Office Web | Here |
| --- | --- |
| Tailwind CSS v4, standalone binary | Same binary, fetched by `tools/fetch-tailwind.sh` |
| daisyUI v5 as a Tailwind plugin | Vendored under `desktop/assets/vendor/daisyui` |
| Heroicons v2 as generated mask classes | Same plugin, over a curated icon set |
| Inter, self-hosted, two subsets | The same two `.woff2` files |
| The `@theme` type scale | Copied verbatim |
| Both OKLCH themes | Copied verbatim |
| `data-theme` dark variant | Already how `boot.js` worked — no adaptation |
| Micro-interactions in CSS | Copied, minus the `phx-*` variants |
| Phoenix LiveView, HEEx | Not portable. The console is offline; it keeps `invoke()` |
| `shared_components.ex` | Ported *as structure* into `frontend/ui.js` |

The last row is the one that needed a decision. HEEx has function components and plain JS
does not, so `ui.js` holds the class strings as named constants and returns markup for the
shapes used on more than one screen. It is not as good as compile-time components; it is
what keeps every table and badge in this app looking like every other one, and like the
web app's.

## No Node, still

Adding Tailwind did not add a JavaScript toolchain. There is no `package.json`, no
`node_modules` and no `npm install`. The compiler is a single binary with a JS runtime
inside it, and daisyUI and the icons are committed to this repo — so exactly one thing is
ever downloaded, once, by a script that skips the work if it is already there.

**The compiled stylesheet is committed.** `cargo build` and `cargo test` do not run
Tauri's `beforeBuildCommand`, so a generated-only stylesheet would mean every plain cargo
build produced an unstyled app and every test ran against one. Committing
`frontend/tailwind.css` keeps building the console a one-command job for anyone who is not
editing styles. `build-release.sh` recompiles it (minified) before bundling, so a release
can never ship whatever CSS happened to be committed last.

## Two things worth knowing

**Tailwind only emits classes it can literally see.** It scans `index.html` and `app.js`,
and a class assembled at runtime — `"badge-" + kind` — is invisible to it and renders
unstyled with no error. Every class name in `ui.js` is therefore a complete literal, and
the variant maps there pick from a fixed set rather than interpolating.

**`[hidden]` needed rescuing.** The shell toggles panes and menus with the attribute while
the same elements carry `flex`, and a class beats the UA stylesheet. `app.css` restores
`[hidden] { display: none !important }` — without it the attribute silently stops meaning
anything. The same applies to `.pane` / `.pane.is-active`, which `showPane()` toggles: the
rule has to live in a stylesheet because a class is what the JS has to work with.

## What this commit changes on screen

The dark top bar is gone. The brand moves to the head of the sidebar and the account to
its foot, with a thin bar over the page holding the connection state and the theme
toggle — the Back-Office Web's shell, arrived at by copying it rather than by taste.

Converted: the sign-in screen, the first-run setup screen, the sidebar, the header bar,
the shortcut bar and the status bar. The nav icons are Heroicons rather than hand-drawn
paths.

Not yet converted: the Billing, History, Settings and Users panes, and the printable
sheet. `theme.css` and `styles.css` still load *after* the new stylesheet so those panes
keep working unchanged; they come out pane by pane, and nothing new should be added to
them. The printable sheet keeps its own paper palette either way — a bill on paper is not
themed.

## Verified

Wiped the database and drove the app under Xvfb: the setup screen, the shell after
creating an owner, and the sign-in screen on a restart, in both themes. 86 tests pass.

Not verified: a full `tauri build`. The stylesheet step was added to `build-release.sh`
rather than to `beforeBuildCommand`, precisely because a bundle build is not something
this environment can exercise end to end, and an untested hook in the packaging path is
worse than an explicit line in the script.
