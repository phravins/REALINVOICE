# Architecture

## One core, three runtimes

`realinvoice-core` owns the schema, the GST rules, invoice numbering and every write
path. The three runtimes are presentation layers over it, so a rate change or a numbering
fix lands once:

| Runtime | Component | Audience | Stage |
| --- | --- | --- | --- |
| Tauri desktop | Office Console | billing counter | 1 |
| Phoenix LiveView | Back-Office Web | owner, accountant | 2 |
| Ratatui TUI | Logistics Desk | dispatch, stores | 3 |

A runtime that reimplements billing logic is a bug. The Office Console's Tauri commands
are one-line wrappers for exactly this reason.

## Offline-first, sync-later

Each node bills against its own local SQLite file and never blocks on the network. Every
write to `customers`, `items`, `invoices` or `invoice_lines` also inserts a row into
`sync_queue` **inside the same transaction**, carrying the full row as JSON:

```
sync_queue(id, table_name, row_id, op, payload_json, created_at, synced_at)
```

Because the queue row and the change it describes commit together, the queue can never
describe a write that did not happen, or miss one that did. Nothing drains the queue yet —
`synced_at` stays `NULL` and `invoices.sync_status` stays `'pending'`. The worker that
sends them, and the conflict rules it needs, are a later stage.

## GST

One decision drives the split: the customer's `place_of_supply` against the seller's home
state (`TN` until a settings table exists).

- Same state → intra-state → CGST and SGST, each half the item's rate.
- Different state → inter-state → IGST at the full rate.

Tax is computed per line and summed, so one invoice can carry 5%, 18% and 28% lines
correctly. Money rounds to paise, half away from zero, at each line and again on the
totals.

## Invoice numbering

`RI-YYYY-NNNN`, sequential within an Indian financial year (1 April – 31 March), where
`YYYY` is the year the FY starts in. 2026-09-11 and 2027-02-11 both draw from the FY 2026
counter; 2027-04-01 starts `RI-2027-0001`. The next number comes from the highest already
issued in that FY, so a lexical `MAX(invoice_no)` over the zero-padded numbers is enough.

## Money representation

Amounts are `f64` rounded to paise at every boundary, which holds comfortably for SME
invoice magnitudes. If a later stage needs exactness under aggregation across a full
year's books, integer paise is the migration — the rounding is already funnelled through
`gst::round_money`, so there is one place to change.
