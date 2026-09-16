# Office Console — Stage 19 (discounts)

Per-line and per-invoice discounts, on top of the price-list rate from stage 18. This
changes what an invoice comes to, so the arithmetic was worked out by hand before any of
it was written, and the hand working is in the tests.

## The tax rule, and why there is no toggle

Under GST a discount **known and disclosed at the time of sale** is a trade discount: it
reduces the taxable value, and tax is charged on what the customer was actually charged. A
discount handed over *after* the invoice is a legally distinct thing that generally cannot
reduce GST liability retrospectively.

Every discount here is the first kind. `DiscountType` has exactly three variants — `none`,
`percentage`, `flat` — and no "after tax" option, deliberately. A toggle beside the
correct behaviour is an invitation to pick the wrong one, and switching it would not be a
preference, it would be a filing position that needs an accountant behind it.

## Order of operations

Per line: `qty × rate`, less that line's own discount, less its share of the invoice-level
discount — and **then** tax, on what is left. Then the buckets are summed.

## Why the invoice discount is apportioned across lines

It would be simpler to take the invoice discount off the subtotal in one go and tax the
result. That only works if every line carries the same GST rate. This catalogue ships items
at 5%, 18% and 28%, so a single discounted subtotal has no one rate to be taxed at — the
simple version would have to pick one and would be wrong on every mixed-rate bill.

So the discount is split across the lines in proportion to what each contributes to the
subtotal **after** line discounts — which is the figure the invoice discount is calculated
on — and each line is then taxed at its own rate on its own reduced base. Each line's share
is stored, so the tax on any line is re-derivable from that line alone.

Rounding the shares to paise leaves a residue of a paisa or two. It lands on the last line
that has anything to discount, so the shares sum to exactly the invoice discount rather
than to nearly it. An invoice whose parts do not add up to its total is worse than one
whose last line absorbs a paisa.

**This is the part most likely to want a real accountant's review**, and it is commented as
such in `core/src/gst.rs`.

## The worked example, by hand

```text
  L1  1 x 45,000.00 @ 18%, no discount     gross 45,000.00
  L2  5 x 12,000.00 @ 18%, 10% off         gross 60,000.00, less 6,000.00 = 54,000.00
  invoice: flat 1,000.00 off

  pre-discount subtotal  45,000 + 60,000                        = 105,000.00
  after line discounts   45,000 + 54,000                        =  99,000.00
  invoice discount apportioned over that 99,000.00:
      L1  1,000 x 45,000/99,000 = 454.5454...                   =     454.55
      L2  1,000 x 54,000/99,000 = 545.4545...                   =     545.45
  taxable  L1 44,545.45   L2 53,454.55                          =  98,000.00
  tax at 9% + 9%:
      L1  44,545.45 x 9% = 4,009.0905  ->  4,009.09
      L2  53,454.55 x 9% = 4,810.9095  ->  4,810.91
                                              CGST = SGST       =   8,820.00
  grand total  98,000 + 8,820 + 8,820                           = 115,640.00
```

A second, nastier case — three lines at 28%, 5% and 18%, a flat line discount and a
percentage invoice discount — is worked out the same way in the tests, because a
single-rate example would pass even if the per-line split were wrong.

## What is stored, and why

`invoice_lines` gains `discount_type`, `discount_value`, `discount_amount`,
`invoice_discount_share` and `taxable_value`. `invoices` gains `invoice_discount_type`,
`invoice_discount_value`, `invoice_discount_amount`, `discount_amount` and
`pre_discount_subtotal`.

Every one of those is **frozen at billing time**. Nothing recomputes them. A price-list
change, a rate correction or a repaired rounding rule next year must never move the numbers
on a document the customer is already holding — which is exactly what the verification
below tests.

`line_total` keeps its old meaning, `qty × rate` before discount, so every historical line
is still correct and the migration only has to set `taxable_value = line_total` for rows
that were never discounted. `invoices.subtotal` is now the **taxable** value, with
`pre_discount_subtotal` beside it, so a saved invoice always states both what it would have
cost and what was charged.

## The approval gate

A cashier may discount up to a threshold on their own; past it, an owner's username and
password are required before the invoice will save. The threshold is a setting
(`discount_approval_threshold_pct`, default 15), editable by an owner under Settings,
because different shops draw the line in different places.

**The gate measures the total discount — line and invoice together — against the
pre-discount subtotal.** The brief asked for invoice-level discounts to be gated; gating
only those would leave the obvious way round open, since a cashier could take 90% off every
line and never touch the control that asks for a password. What an auditor cares about is
how much came off the bill, so that is what is measured.

Approval is credentials, not a flag: a boolean the frontend sets is a gate the frontend can
open. They go through the same rate-limited sign-in path as the login screen, and the check
runs in Rust inside `create_invoice`, so a hand-made payload that skips the prompt is
refused just the same. A cashier cannot approve themselves.

## On screen

- Each row carries a collapsed discount control under its Rate — an icon until the row
  actually has a discount, because a discount box on every line is a discount box nobody
  reads. One editor is reused by every row.
- The invoice-level control sits beside the summary, with a live reading of what percentage
  of the bill has come off and whether that needs an owner.
- The summary panel shows **Subtotal (before discount) → Discount → Taxable Value → CGST /
  SGST (or IGST) → Grand Total**, with the discount in red and never folded into another
  figure. A cashier who cannot say why the total is not the shelf price cannot answer the
  customer in front of them.
- The history detail and the printed sheet show the same breakdown, read from the stored
  row. The sheet grows a Discount column only when there is one.

## Verified end to end

Driven through the real app under Xvfb, against a figure worked out in a spreadsheet first:

```text
  RACK-42U-PRO   1 x 45,000.00 @18%, 10% off      45,000.00 -  4,500.00 = 40,500.00
  TMT-12MM      20 x    620.00 @18%, flat 1,200   12,400.00 -  1,200.00 = 11,200.00
  invoice: 5% off 51,700.00 = 2,585.00, split 2,025.00 / 560.00
  taxable  38,475.00 + 10,640.00                                        = 49,115.00
  CGST = SGST                                                           =  4,420.35
  grand total                                                           = 57,955.70
```

The panel showed **57,400.00 / −8,285.00 / 49,115.00 / 4,420.35 / 4,420.35 / 57,955.70** —
every figure to the paisa. Saved as RI-2026-0001.

Then the rack's base and Retail rates were changed to 99,000 and 88,000, and the invoice
reopened in History: **unchanged**, still 45,000.00 a unit, still −4,500.00 (10.00%), still
₹57,955.70.

Signed in as a cashier, a 30% invoice discount showed "30.00% of the bill · needs owner
approval" in red and Print & Lock raised the approval prompt. A wrong password was refused
with nothing saved; the owner's password saved it as RI-2026-0002 at ₹367.36
(410.00 − 30% = 287.00, +28% = 367.36).

172 tests pass; clippy is clean.

## Also in this change

**A credit note on a discounted line now credits the discounted price.** `create_credit_note`
priced returns at `qty × rate`, which after this stage would have refunded the sticker
price on a discounted line — handing back money the customer never paid and reversing more
tax than was ever charged. `CreditableLine` gained `effective_rate`
(`taxable_value / billed_qty`), and that is what a credit is priced at. A test returns
every unit of a 20%-discounted line and asserts the invoice nets to exactly zero.

**The quote and the save are now one code path.** The summary panel used to be priced by a
pure function in the desktop crate from rates the screen supplied. It now calls
`Db::quote_invoice`, which resolves the price list and applies discounts exactly as the
save does, so the panel cannot show a figure the save would disagree with. The "quantity
must be positive" rule moved to the save, because a bill being typed has half-entered rows
in it and a cleared quantity box has to price as zero rather than throw the panel away.

**`check_expected_totals` covers the discount figures too**, and with a customer attached
the GST split is decided by *their* place of supply rather than the one the screen sent.

## Not done

An invoice still does not record which price list it was billed from (carried over from
stage 18). Discounts are not reportable yet — Analytics shows revenue, not what was given
away, which is the first thing the reports stage should add. There is no reason or
authorisation trail on a discount: the invoice records that an owner approved it only in
the sense that it exists, not who approved it or why. And `add_line_item` re-apportions the
invoice discount across every line when a line is appended, which is correct but rewrites
rows on a saved invoice — acceptable only because nothing in the UI calls it.
