-- Discounts, per line and per invoice.
--
-- Every column added here is a *frozen* figure: computed once from the discount type and
-- value in force at the moment of billing, then stored. Nothing downstream recomputes
-- them. A price-list change, a rate correction or a repaired rounding rule next year must
-- never move the numbers on a document the customer is already holding.
--
-- Under GST these are trade discounts — known and disclosed at the time of sale — so they
-- reduce the taxable value *before* tax is computed. There is deliberately no "discount
-- after tax" option: a post-invoice discount is a legally distinct thing that generally
-- cannot reduce GST liability after the fact, and offering it as a toggle beside this one
-- would invite a shop to pick the wrong one.

-- 'none' | 'percentage' | 'flat'. Stored as text so a database opened in any SQLite
-- browser reads as what it is.
ALTER TABLE invoice_lines ADD COLUMN discount_type  TEXT NOT NULL DEFAULT 'none';
-- 10 for 10%, or 500 for a flat ₹500 off. Meaningless without discount_type.
ALTER TABLE invoice_lines ADD COLUMN discount_value REAL NOT NULL DEFAULT 0;
-- The rupees this line's own discount took off, computed from the two above.
ALTER TABLE invoice_lines ADD COLUMN discount_amount REAL NOT NULL DEFAULT 0;
-- This line's apportioned share of the invoice-level discount. Stored because tax is
-- computed per line at that line's own GST rate, so the share is part of the tax base and
-- has to be reconstructible from the row alone.
ALTER TABLE invoice_lines ADD COLUMN invoice_discount_share REAL NOT NULL DEFAULT 0;
-- What tax was actually charged on: line_total - discount_amount - invoice_discount_share.
ALTER TABLE invoice_lines ADD COLUMN taxable_value REAL NOT NULL DEFAULT 0;

-- Existing lines were never discounted, so what they were taxed on is what they came to.
UPDATE invoice_lines SET taxable_value = line_total;

ALTER TABLE invoices ADD COLUMN invoice_discount_type   TEXT NOT NULL DEFAULT 'none';
ALTER TABLE invoices ADD COLUMN invoice_discount_value  REAL NOT NULL DEFAULT 0;
ALTER TABLE invoices ADD COLUMN invoice_discount_amount REAL NOT NULL DEFAULT 0;
-- Line discounts plus the invoice discount: the one figure the summary panel shows.
ALTER TABLE invoices ADD COLUMN discount_amount         REAL NOT NULL DEFAULT 0;
-- What the bill would have come to before any discount. Kept alongside `subtotal` (which
-- is now the *taxable* value, after discounts) so a saved invoice always states both what
-- it would have cost and what was charged, without either being derived later.
ALTER TABLE invoices ADD COLUMN pre_discount_subtotal   REAL NOT NULL DEFAULT 0;

UPDATE invoices SET pre_discount_subtotal = subtotal;
