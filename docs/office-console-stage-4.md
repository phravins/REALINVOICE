# Office Console — Stage 4

Invoice history: its own tab, a filterable list, a read-only detail view, and a Reprint
that renders the same printable sheet Print & Lock produces.

## Where it lives

**A new History tab, next to Billing** — not a side panel inside the transaction form.
The billing card already fills the width and most of the height of a 1180×760 window, and
the low-spec machines this targets often run smaller than that; a side panel would have
squeezed the form an operator uses all day to make room for one they open occasionally.
The tab row is now Billing | History | Inventory | Analytics | Sync | Settings.

(The stage-1 "List Today's Invoices" button was already gone — stage 2 replaced the whole
Billing pane with the NEW TRANSACTION card. So this stage adds history fresh rather than
moving an existing control.)

## List

`Db::list_invoices(&InvoiceFilter)` is new in core. `list_todays_invoices` only ever
answered one question, so rather than bolt arguments onto it the filter is its own type:

```rust
InvoiceFilter { from: Option<String>, to: Option<String>, text: Option<String>, limit: Option<u32> }
```

`from`/`to` are inclusive bounds on `invoices.date`; `text` is a case-insensitive
substring matched against **customer name or invoice number**; every field is optional and
an empty filter lists everything, newest first. Each row comes back as an `InvoiceSummary`
— the invoice plus customer name, mobile and line count — so the list renders without a
query per row.

The pane defaults to **Today**, and also offers This week (Monday-start, the week a shop
reconciles against), This month, All time and a custom from/to range. Range and text
compose. A footer totals the filtered set.

### `invoices.created_at` is now local

The list shows a date *and time*. `created_at` existed but defaulted to `datetime('now')`,
which is UTC, while `date` has always been the counter's local day — a time from a
different clock beside a local date misleads. `create_invoice` now stamps `created_at`
explicitly in local time, and it is exposed on the `Invoice` model.

## Detail

Clicking a row (or pressing Enter on it) opens `Db::get_invoice_detail`, which returns the
invoice, the buyer, and the lines with their item code, description and UOM resolved — a
reprint needs all three, and fetching them once beats a lookup per line.

The view is **read-only and marked so**. There is no edit, delete or reopen action, and no
command that would support one: invoices are append-only once saved. Fixing a mistake is a
credit note or cancellation, which is a later feature, not a back door here.

### The summary component is shared, not rebuilt

`renderTotals(element, figures)` is one function with two callers: the billing card feeds
it a live quote from core, the detail view feeds it a saved invoice. Intra-state renders
CGST + SGST, inter-state renders IGST — the caller does not decide, the figures do.
`lineRowsHtml(lines, editable)` is the same story for the item table: with `editable` it
draws the Qty box and remove button for billing, without it the same columns as plain text
for the detail view and the sheet.

Wiring the detail view to these actually simplified stage 2's code — the billing totals
were five hard-coded `<dt>/<dd>` pairs toggled by `hidden`; they are now generated.

## Reprint

Stage 3's Print & Lock saved and locked but produced no printable artefact, so there was no
"existing layout" to reuse — this stage adds one and gives both paths to it:

- **Reprint**, on the detail view, renders from the stored invoice.
- **Print preview**, now on the locked billing card, renders from what the save returned.

Both call `renderPrintable()`, so a reprint is the same document with the same numbers,
read back from SQLite with nothing re-entered and nothing recomputed. The sheet is a
paper-white A4-style document — seller header, invoice number and timestamp, billed-to
block with GSTIN and place of supply, numbered lines with UOM, and the GST split — with a
`@media print` rule that sends the sheet alone to paper.

The **actual printer call is stubbed**: the Print button calls `window.print()`, which is
the webview's own dialog. No printer configuration, paper size or driver selection is
wired up, and the seller name on the sheet is a placeholder until the Settings stage.

## Verified

Four invoices billed through stage 3's flow under Xvfb, varying customer, items and
totals:

| Invoice | Customer | Lines | Grand total |
| --- | --- | --- | --- |
| RI-2026-0001 | Ishta Capital Investments (TN) | rack ×1, licence ×5 | ₹1,23,900.00 |
| RI-2026-0002 | Kaveri Hardware (TN) | cement ×20 | ₹10,496.00 |
| RI-2026-0003 | Deccan Supplies (KA) | licence ×2 | ₹28,320.00 (IGST) |
| RI-2026-0004 | Walk-in Customer (TN) | PVC pipe ×4 | ₹1,347.56 |

1. History listed all four, newest first, with number, date/time, customer, payment and
   grand total, footing to ₹1,64,063.56.
2. Custom range 2025-01-01 → 2025-01-01 narrowed to "No invoices match", 0 in range.
   Back to Today restored all four.
3. Text filter `kaveri` narrowed to RI-2026-0002 alone, composed with the Today range.
4. Opening RI-2026-0001 showed both lines (45,000.00 and 60,000.00) and Subtotal
   ₹1,05,000.00 / CGST ₹9,450.00 / SGST ₹9,450.00 / Grand Total ₹1,23,900.00 — matching
   `db.sqlite` read from a separate process, row for row.
5. Reprint rendered the same figures, and Print preview on the locked billing card
   rendered RI-2026-0004 through the same layout.

## Not in this stage

- Editing, voiding or credit-noting a saved invoice.
- A real print path: paper size, printer selection, or a PDF writer.
- Configurable seller details on the sheet — Settings stage.
- Pagination. The query caps at 500 rows; a busy counter will outgrow that.
- Inventory, Analytics and Sync panes.
