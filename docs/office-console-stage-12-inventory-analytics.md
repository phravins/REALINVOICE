# Office Console — Stage 12 (Inventory and Analytics)

The two panes that had been placeholders since stage 1. They are built on the design
system the console now shares with the Back-Office Web, so neither invented anything
visual: the tables, cards, empty states and charts are the vocabulary that already
existed.

## The rule that shaped both

**No money is computed in JavaScript.** It is the rule the billing screen has followed
since stage 2, and Analytics is where it would have been easiest to break — summing a list
of invoices in a `reduce()` is one line. Instead every figure on that pane is added up by
SQLite inside core:

| Core | What it answers |
| --- | --- |
| `sales_summary(&DateRange)` | invoice count, taxable value, CGST/SGST/IGST, tax total, billed |
| `daily_totals(&DateRange)` | one row per day that had billing |
| `top_items(&DateRange, limit)` | what sold, by revenue |
| `payment_mix(&DateRange)` | how customers paid |

The pane's JavaScript formats what comes back and nothing else. The tests assert the parts
reconcile with the whole — `subtotal + tax_total == grand_total`, the days sum to the
summary, and the payment mix covers every invoice exactly once — because a figure a
shopkeeper might file a return against should not be able to drift from the invoices it
came from.

`analytics` is deliberately **one** command returning all four. Four commands would be
four locks of the same connection and four chances for the screen to show figures from
four different instants.

## Inventory

The catalogue the billing screen prices from: list, search, add, and edit.

Editing is the same call as adding, because core keys items on `item_code` — a shop
changing a price is doing the same thing as a shop adding the line for the first time. The
code field is read-only while editing: changing it would quietly create a second item
rather than rename this one.

Unlike accounts, the catalogue **is** queued for sync. It is shared business data, not a
per-machine credential. The test asserts an `insert` then an `update` land in
`sync_queue` across an add and an edit.

`Db::list_items` is separate from `Db::search_item` rather than a widening of it.
`search_item` feeds the billing picker and is capped at 50 on purpose; this pane is where a
shop with 900 items expects to see 900 items.

The form guards the money fields before they reach SQLite — a negative rate, a NaN, a tax
rate of 900%. Those are typos, and the person who made one is standing at the till. Both
boundaries stay legal: zero-rated goods and free samples are real.

## The charts

Ported from `dashboard_components.ex`: a grouped bar chart of tax by day and an SVG donut
of the payment mix, written as functions in `ui.js` that return markup. No charting
library — they inherit the app's own tokens, which is why they do not look bolted on, and
it is one fewer dependency on a machine that has to keep billing with no network.

Two details carried over deliberately. The bar chart's ceiling is a round number at or
above the tallest bar, so the gridline labels read as money rather than as whatever the
peak happened to be; and a day with billing but a tiny amount still draws a hairline,
because a zero-height bar reads as missing data rather than as a quiet day. The chart shows
the last 14 billed days — beyond that the bars are too narrow to read, and a till looking
at a year wants the table underneath.

## The bug this stage produced

The charts rendered with no bars and no ring on the first run. Nothing errored.

`app.css` listed `index.html` and `app.js` as Tailwind's sources but not `ui.js`, which is
where every chart class lives — so `h-64`, `size-40` and the donut's `stroke-*` classes
were never emitted. This is exactly the failure mode `DEVELOPMENT.md` warns about, arriving
by the other route: not a class assembled at runtime, but a whole file the compiler was
never pointed at. `ui.js` is now a source, and the comment there says why every file that
writes a class name has to be listed.

## Verified

Drove the app under Xvfb in both themes. Inventory: the seven seeded items listed and
sorted, a search miss showing "0 of 7 item(s) match", an item added and defaulting to NOS,
then edited from ₹180 to ₹195 — confirmed in SQLite as one catalogue row with an `insert`
and an `update` queued. Analytics: eight invoices across six days, with every figure on
screen cross-checked against the same query run directly against the database
(₹3,26,600 taxable, ₹65,348 tax, ₹3,91,948 billed, reconciling), the bar chart scaled to a
₹20,000 ceiling, the donut's shares summing to 100%, and the empty state on a fresh
database hiding every panel while leaving the range control to widen.

97 tests pass; clippy is clean.
