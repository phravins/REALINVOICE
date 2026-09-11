# Office Console — Stage 2

The throwaway stage-1 test pane is gone; the Billing tab is now the real **NEW
TRANSACTION** card. The shell around it — title bar, node name, `[Connected]` badge, tab
row — is unchanged.

Print & Lock still does **not** save. It assembles the payload, logs it, and shows it.
Wiring it to `create_invoice` is stage 3.

## The card

| Section | Behaviour |
| --- | --- |
| Header | Card title plus the mobile search, moved up from stage 1. Enter searches. |
| Customer line | `Name — GSTIN: … · mobile · place of supply XX`. `place_of_supply` is held and drives the GST split. |
| New customer | Appears inline when a mobile resolves to nobody: name, mobile (prefilled), GSTIN (optional), place of supply (defaults `TN`) → `create_customer`, then attaches the result. |
| Item rows | Item Code, Description, Qty, Rate (₹), Tax %, Total (₹), remove. Qty is editable inline. |
| Add Item | Opens a picker over `search_item`; picking adds a row prefilled from the item master with Qty 1. Escape closes it. |
| Summary | Subtotal, then CGST + SGST **or** IGST, then Grand Total. Re-quoted on every change. |
| Print & Lock (F5) | Assembles the payload, `console.log`s it, and shows it in the preview box. Writes nothing. |

## No money is computed in JavaScript

Row totals, the GST split and the grand total all come back from the `quote_invoice`
command, which calls `realinvoice_core::gst::compute_totals` — the same function that
prices the invoice when it is saved. The frontend only formats what core returns. That
is why the summary panel re-invokes on every keystroke in a Qty box: the call is a pure
function with no database access, and paying for the IPC is cheaper than keeping a second
implementation of GST in `app.js` that can drift from the one that bills.

### New commands

```
create_customer(payload: NewCustomerPayload) -> Customer
quote_invoice(placeOfSupply: String, lines: [{qty, rate, tax_rate}]) -> InvoiceQuote
```

`InvoiceQuote` carries `home_state`, `intra_state`, `line_totals` (per row, in order) and
the five money fields. `Db::create_customer` is new in core: a strict insert that refuses
a mobile number already on file, trims its input and upper-cases the state code. Like
every other write, it queues a `sync_queue` row in the same transaction.

## Seed data added

- `RACK-42U-PRO` — 42U Server Rack Pro, ₹45,000.00, 18%, NOS
- `ABCOS-ENT-LIC` — aBCOS Enterprise Lic, ₹12,000.00, 18%, LIC
- Customer `9600011223` — Ishta Capital Investments, GSTIN `33AAAAA0000A1Z1`, TN

## Verified

Run under Xvfb, driven with `xdotool`, against a fresh config dir:

1. `9600011223` resolves to **Ishta Capital Investments — GSTIN: 33AAAAA0000A1Z1 ·
   9600011223 · place of supply TN**.
2. Both sample items added from the picker; licence Qty edited to 5. The panel showed
   **Subtotal ₹1,05,000.00 · CGST ₹9,450.00 · SGST ₹9,450.00 · Grand Total
   ₹1,23,900.00**, with row totals ₹45,000.00 and ₹60,000.00 and no IGST line.
3. Removing the rack row re-priced live to Subtotal ₹60,000.00, CGST/SGST ₹5,400.00,
   Grand Total ₹70,800.00.
4. An unregistered mobile opened the New-customer form; registering Anand Electricals
   attached it immediately, and the row plus its `sync_queue` insert were in the file.
5. A Karnataka customer switched the panel to a single **IGST ₹2,160.00** line
   (`Inter-state · TN → KA`), CGST and SGST hidden.
6. F5 fired Print & Lock without reloading the shell.
7. After several Print & Locks, the database still held **0 invoices and 0
   invoice_lines** — the button writes nothing, as intended for this stage.

### The logged payload

Captured from the worked example. `invoice` is exactly core's `NewInvoice`, which is what
stage 3 passes to `create_invoice`; a test pins this shape.

```json
{
  "customer": { "id": 4, "name": "Ishta Capital Investments", "gstin": "33AAAAA0000A1Z1",
                "mobile": "9600011223", "place_of_supply": "TN" },
  "invoice": {
    "customer_id": 4,
    "date": null,
    "payment_type": "cash",
    "lines": [
      { "item_id": 1, "qty": 1, "rate": 45000, "tax_rate": 18 },
      { "item_id": 2, "qty": 5, "rate": 12000, "tax_rate": 18 }
    ]
  },
  "lines_display": [ { "item_code": "RACK-42U-PRO", "description": "42U Server Rack Pro",
                       "uom": "NOS", "qty": 1, "rate": 45000, "tax_rate": 18,
                       "line_total": 45000 }, "…" ],
  "totals": { "home_state": "TN", "intra_state": true, "subtotal": 105000,
              "cgst": 9450, "sgst": 9450, "igst": 0, "grand_total": 123900 }
}
```

`lines_display` and `totals` are display copies of what core already priced — stage 3 does
not send them, because `create_invoice` recomputes both. They are in the payload so the
saved invoice can be diffed against what the operator was shown.

## Notes for stage 3

- A **payment type** selector was added to the summary row. `create_invoice` requires
  `payment_type` and nothing in the stage-2 brief supplied one, so the payload would have
  been incomplete without it. Cash / UPI / Card / Credit, defaulting to Cash.
- Rate and Tax % are display-only. Core already accepts per-line overrides
  (`NewInvoiceLine.rate` / `.tax_rate`), so making them editable is a frontend change.
- A cleared Qty box prices as zero rather than blanking the panel; the box is outlined in
  red and `create_invoice` would reject the line, so stage 3 should block the save.
- Anything that changes the transaction clears the logged payload and its confirmation —
  a stale total next to a fresh one is how a counter mis-bills.
