# Office Console — Stage 8 (flat shell, sidebar, unboxed surfaces)

A restructure to the flatter SaaS shape of the OpenCloud reference: dark top bar, flat
sidebar, content on the page rather than in floating cards, and settings as label/value
rows. Four frontend files changed — no command, no core, no business logic, and all 86
tests pass untouched.

## Top bar

One solid dark band flush to the window edges, not a bordered panel. It keeps its own
colours in **both** themes (`--topbar-bg`, `--topbar-ink`, `--topbar-hover`) because it is
chrome, not a surface the content sits on — the reference does the same.

Left is the wordmark. Right is a connection status icon, the theme toggle, and a circular
avatar showing the signed-in user's initial on `--avatar-bg` (teal, so a person does not
read as another indigo action). The avatar opens a small menu with the user's name and
role, **Account & About**, and **Sign Out** — which is where Sign Out now lives.

**The centre is deliberately empty.** A global search belongs there once there is
something worth searching across every pane; wiring one now would be a new feature, which
this pass excludes. The brief allowed either.

## Sidebar

The horizontal tab row is now a vertical nav: icon + label per item, on the same flat
ground as the content (`--bg-app`), separated only by a hairline. The active item takes a
soft `--surface-hover` wash, a bolder label and an accent icon — no border, no box.

Its foot carries the app name and version (read from `app_info`, so it follows
`tauri.conf.json`) over a muted `Local — not yet synced` line, in the slot where the
reference shows `OpenCloud 7.2.4 stable / Up to date`.

## Content — de-boxed

`.card` no longer paints anything: no background, no border, no shadow, no padding. The
billing screen's sections sit on the flat page, separated by the dividers they already
had. Where grouping genuinely helps, it is a soft `--surface-2` fill with a radius and
**no outline** — the totals block, the item picker and the history filter toolbar all use
that treatment now.

Two exceptions, both deliberate: the inline New-customer form keeps its accent edge
because it is an active form demanding attention, and the account menu and print overlay
keep elevation because they genuinely float above the page.

A saved invoice no longer gets a full green border. It gets a green left rail — same
signal, no box.

### Empty states

The billing table and the history list now show an illustration, a title and a next step,
in place of a single grey line:

- **No items added yet** — "Press `F2` or use Add Item to start a bill"
- **No invoices match** — "Try a wider date range, or clear the search"

## Settings as rows

The About pane is now a page, not a card: a title, then label-left / value-right rows
split by thin dividers, in two sections — **Account Information** (username, display name,
role, node, theme) and **About** (application, version, identifier, build date, database).
No bordered inputs, because nothing there is editable.

There is **no Edit button**. The brief asks for one "where relevant"; nothing on this page
can be edited until the user-management screen exists, and a button that does nothing is
worse than no button.

## Login — nothing containing it

The card is gone. The form is the wordmark, two fields and a button, centred with room
around them. Inputs are underlined rather than boxed — transparent background, a single
bottom border that thickens and takes the accent on focus. The only edged thing left is
the first-run credentials notice, which is a notice and needs an edge.

## Tokens

New: `--topbar-bg`, `--topbar-ink`, `--topbar-ink-dim`, `--topbar-hover`, `--avatar-bg`,
`--avatar-ink`, `--sidebar-w`, `--topbar-h`. Retired `--titlebar-h` and `--tabs-h`.
`styles.css` still names no colour of its own.

## Narrow windows

`≤980px` narrows the sidebar to 180px. `≤760px` collapses it to a 56px icon rail — labels
and the version footer hide, the icons stay.

## Verified

Every screen walked in **both themes at both sizes** (1180×760 and 760×520): login, top
bar, sidebar, billing empty and with rows, item picker, locked invoice, history, account
menu, and the About page. Nothing is boxed that does not need to be, and nothing
overflowed or clipped.

Four bugs found while walking it and fixed: `loadNodeStatus` still wrote to the deleted
`#node-label` and threw on every sign-in; the wordmark rendered dark-on-dark because it
takes `currentColor` and the bar needed `--topbar-ink`; the sidebar version only appeared
after opening Settings; and `overflow: hidden` on the top bar clipped the account menu
that hangs off it.

## Not in this pass

No new business features. The status icon is still a stub, F9 is still a placeholder, the
UPI panel is still a placeholder, and the printed sheet keeps its own paper palette.
