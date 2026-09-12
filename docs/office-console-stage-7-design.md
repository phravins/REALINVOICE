# Office Console — Stage 7 (design system, theming, branding)

Light and dark themes, a design-token layer, a wordmark, and an About pane. No business
logic changed: GST, numbering and `create_invoice` are untouched, and all 86 tests pass.

> **One deviation, stated up front.** The brief says not to touch core, then asks for the
> theme to persist "via a Tauri command that writes to a… table in core". Core gained a
> `settings` key/value table and two accessors — infrastructure only. Nothing in the
> billing path was opened.

## 1. Tokens

`desktop/frontend/theme.css` is now **the only file in the app that names a colour**.
`styles.css` names none: every rule references a token, which is what lets both themes
flip from one attribute.

| Group | Tokens |
| --- | --- |
| Background layers | `--bg-app`, `--bg-sunken`, `--surface`, `--surface-2`, `--surface-hover`, `--field-bg` |
| Text | `--text`, `--text-secondary`, `--text-muted`, `--text-on-accent` |
| Borders | `--border`, `--border-strong` |
| Accent | `--accent`, `--accent-hover`, `--accent-soft`, `--accent-text` |
| Semantic | `--success`, `--warning`, `--danger`, each with a `-soft` fill |
| Spacing | `--s1..--s6` = 4 / 8 / 12 / 16 / 24 / 32 |
| Radius | `--r-sm` 6 (controls), `--r-md` 10 (cards), `--r-lg` 14 (login, overlays) |
| Elevation | `--shadow-sm/md/lg`, `--focus-ring`, `--scrim` |
| Type | `--font-ui`, `--font-mono`, `--t-h1/h2/body/small/caption`, `--lh-tight/body` |

**The accent is indigo** — `#6366f1` on dark, `#4f46e5` on light. Confident without being
loud: this is a ledger, not a dashboard, and the one place it shouts is the grand total.

Each palette is written **exactly once**. There is no duplicate copy inside a
`prefers-color-scheme` media query to drift out of step — `boot.js` sets `data-theme`
before first paint instead, so the OS default costs one line of JavaScript rather than a
second palette.

Financial figures use `font-variant-numeric: tabular-nums` wherever they appear (item
rows, summary, history, print sheet), so digits line up down a column.

## 2. Theme toggle

A sun/moon button in the title bar, showing the theme you would switch *to*. Resolution
order:

1. the saved choice, from core's `settings` table (`ui.theme`);
2. the operating system, via `prefers-color-scheme`;
3. dark.

Persistence goes through two new commands, `get_theme` / `set_theme`, onto
`Db::get_setting` / `set_setting`. Like `users`, settings are **not** queued for sync: one
till's display preference is not something the back office should receive, let alone
impose on another node.

`localStorage` holds a copy, used only by `boot.js` to paint the first frame in the right
theme. Core's table is the authority and `app.js` reconciles against it a moment later —
so a cleared cache costs a flash, never the preference.

Until someone actually chooses, nothing is saved and the app keeps following the OS.

## 3. Branding

One `<symbol id="ri-logo">` in `index.html`, referenced by `<use>` in the title bar, on the
login screen and in About. One definition, three sizes — the treatments cannot drift.

Its shapes are styled with inline `style` attributes rather than classes, because a `<use>`
renders into a shadow tree that outside selectors cannot reach. Custom properties and
`currentColor` *do* inherit into it, so the mark takes `--accent` and the wordmark takes
the surrounding text colour, and the logo themes itself.

Swapping in a real logo means replacing the contents of that one symbol. No layout code
changes.

## 4. About

The Settings tab's "Coming soon" stub is now a real pane: application name, version,
bundle identifier, build date, node, signed-in user, current theme and database path.

Every field is read from the running build:

- **Version** comes from `app.package_info().version` — Tauri's own idea of the app
  version, which is `tauri.conf.json`'s `version` field and the number that goes into the
  bundle. Bumping it there is the only edit needed; verified by bumping to **0.2.0** for
  this release and watching About follow without a second change.
- **Build date** is stamped by `build.rs` into `REALINVOICE_BUILD_DATE`, honouring
  `SOURCE_DATE_EPOCH` so reproducible builds stay reproducible.

## 5. Re-fitting

Cards are elevated surfaces (`--shadow-md`) rather than flat outlines — on light that is
what lifts a card off the page, on dark it is what stops every panel reading as one sheet.
Tabs took a soft accent fill plus the inset underline; the Connected badge became a status
pill with a coloured dot so the label stays readable in both themes; inputs gained hover
and focus-ring states.

The structural work from the previous pass is unchanged and still holds: the shell grid
with `minmax(0, 1fr)`, `table-layout: fixed` on every table, the 760×520 window minimum
and the reflow breakpoints.

## Verified

Every screen walked in **both themes at both sizes** (1180×760 and 760×520): login,
title bar, tab row, billing card with a long customer name and long item description,
item search, invoice history with a five-digit number, read-only detail, print preview,
and About. Nothing overflowed, clipped or overlapped in any of the four combinations.

- **Theme persistence:** chose dark, closed the app entirely, relaunched — it came back
  dark against an OS that prefers light, with `settings` holding `ui.theme = dark`.
- **Version:** About showed `0.1.0`, then `0.2.0` after the bump in `tauri.conf.json`,
  with no frontend edit.
- **Logo:** identical mark and wordmark on the login screen, title bar and About.

Three bugs were found and fixed during the walkthrough: the logo rendered as a black
block (CSS cannot reach into a `<use>` shadow tree); inputs read as disabled on light
(they were painted with the app background, now `--field-bg`); and the Connected badge
kept its stage-1 brackets because JavaScript overwrote the markup.

## Not in this pass

No new business features. The `[Connected]` badge is still a stub, F9 is still a
placeholder, the UPI panel is still a placeholder chequer, and the printed sheet keeps its
own paper palette in both themes — a reprint must not change colour because the operator
prefers dark mode.
