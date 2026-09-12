# Office Console — Stage 6 (UI pass)

No features, no fields, no screens. One layout system, applied to everything that already
existed. Two files changed: `desktop/frontend/styles.css` and the window's minimum size in
`tauri.conf.json`. No JavaScript, no command, no core change — all 83 tests pass untouched.

## What was actually wrong

The stylesheet had grown by appending a section per stage, and stage 5 had left a trailing
"polish" block that re-declared `.card`, `.items td`, `.totals` and `.tot-row--grand`
further down the file — a view's spacing depended on which rule won, which is exactly how
the drift happened. Concretely, before this pass:

- At **820px wide the card spilled off the right edge** and its contents were
  unrecoverable — no scrollbar, just gone. `.totals { min-width: 320px }` and a
  `width: 220px` search input forced the card wider than the window, and no flex child had
  `min-width: 0` to let it shrink.
- At the default size the **card's bottom border was clipped** behind the footer, and a
  scrollbar gutter had appeared over the layout: `main { flex: 1 }` inside a flex column
  with no `min-height: 0`.
- Table columns were sized on `th` under `table-layout: auto`, so **columns moved as row
  content changed length**.
- Buttons differed in height between views; padding ran 6/7/8/9/10/12/14/16/18/20/22px
  with no scale behind it.

## The system

Everything is built from tokens declared once in `:root`:

| Token group | Values |
| --- | --- |
| Spacing | `--s1..--s6` = 4 / 8 / 12 / 16 / 24 / 32px. Nothing in the file uses a spacing value off this ladder. |
| Surfaces | `--bg`, `--panel`, `--panel-2`, `--line`, `--line-soft` |
| Ink | `--ink`, `--ink-dim`, `--ink-faint` |
| Accents | `--accent`, `--accent-ink`, `--ok`, `--err` |
| Radii | `--r-sm` 4px, `--r-md` 6px |
| Controls | `--control-h` 34px — *every* input, select and button |
| Chrome | `--titlebar-h`, `--tabs-h` |

Layout is flexbox throughout, with grid for the two genuinely two-dimensional forms (the
new-customer fields and the first-run credentials block). The only absolute positioning
left is the two overlays that really do sit above the page.

### Shell

```css
.shell {
  display: grid;
  grid-template-rows: var(--titlebar-h) var(--tabs-h) minmax(0, 1fr) auto auto;
  height: 100%;
}
```

`minmax(0, 1fr)` is the whole fix for the clipped card: it lets the content row shrink
below its content's height, so `main` scrolls inside itself instead of pushing the
footers off the bottom. `html, body { overflow: hidden }` means the window itself never
scrolls — only the pane does.

### Tables

One treatment shared by the billing rows, the history list and the read-only detail, so
the three cannot drift apart:

- `table-layout: fixed` — widths come from the header cells and nothing else, so a column
  cannot move because a description got longer.
- Every column has a width except the one free-text column, which absorbs the remainder.
  That is also why the same rules serve the billing table's seven columns and the detail
  table's six.
- Numeric columns right, text columns left, header and body alike; `tabular-nums`
  everywhere figures appear, so digits line up down the column.
- Long values wrap (`overflow-wrap: anywhere`) rather than being clipped or spilling.
- Below `min-width: 660px` the `.table-wrap` scrolls horizontally — the panel's own
  borders stay exactly where they are.

### Summary panel

`.totals` is a bordered block, `margin-left: auto`, capped at 340px, clear of the card
edge, with the Grand Total separated by a top border and carrying the only bold accent
figure in the card.

### Buttons

`.btn` sets height, radius and padding once. `--primary` and `--lock` change colour and
weight only, never size, so a button is the same button wherever it appears.

## Narrow windows

The window's minimum was **900×600, which is wider than the 1024×768 and 800×600 panels
these billing machines run** — the app could not be made small enough for the problem to
show, let alone be fixed. It is now 760×520, and the layout reflows above it:

- **≤900px** — the summary stacks (payment above totals), the customer search takes its
  own full-width row.
- **≤700px** — cards and the content pane drop to 12px padding, the `[Node: POS-01]` chip
  hides (the least load-bearing thing on the title bar), filters go full-width, and the
  fixed table columns tighten so the free text column keeps a readable width instead of
  wrapping one word per line.

Title-bar items truncate with an ellipsis rather than crowding; the tab row scrolls
sideways rather than compressing tabs.

## Verified

Walked end to end at **1180×760** and at the new **760×520** minimum — login, billing,
item search, history, detail, print preview, locked invoice and a placeholder pane — with
a long customer name ("Sri Venkateswara Agencies & Distributors (Coimbatore Main Branch)
Private Limited"), a long item description, and a five-digit invoice number
(`RI-2026-10042`) in play throughout. Nothing overflowed, clipped or overlapped at either
size; where space ran out, the panel scrolled inside its own borders.

Four issues were found *during* that walkthrough and fixed:

1. The item description was squeezed to one word per line at 760px — fixed columns now
   tighten under 900px.
2. `flex: 1 1 260px` on `.summary-left` applied to its *height* once the summary stacked,
   opening a dead gap above the totals.
3. `white-space: nowrap` on table headers made "GRAND TOTAL (₹)" overflow its column;
   under `table-layout: fixed` a wrapped caption cannot change a column width, so headers
   wrap now.
4. The print sheet had inherited `table-layout: fixed` and was breaking amounts across
   lines (`12,345.6` / `7`). It is a document, not a responsive layout: it keeps its
   820px page width, the overlay pans, and amounts never wrap.

## Deliberately unchanged

The dark palette, every command, every core function, and all JavaScript. The stage-5
`[Connected]` badge is still a stub, F9 is still a placeholder, and the UPI panel is still
a placeholder chequer rather than a real QR code.
