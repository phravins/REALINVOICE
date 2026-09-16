-- The customer ledger: what was billed, what was actually received, and the difference.
--
-- Until now `invoices.payment_type` was only a label. It said "cash" because somebody
-- picked cash at the counter, not because any money had changed hands. Real shops sell on
-- account — "put it on my tab" — and take part of a bill now and the rest next week, and
-- neither of those had anywhere to live.
--
-- An invoice records what was **billed**. A payment records what was **received**. They
-- are separate rows because they are separate events, and because a payment can arrive
-- weeks later, against no particular bill, from a customer clearing a running balance.

CREATE TABLE payments (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    customer_id        INTEGER NOT NULL REFERENCES customers (id),
    -- NULL means a payment against the account rather than one bill: somebody handing
    -- over 10,000 towards whatever they owe. Those reduce the customer's balance without
    -- touching any single invoice.
    invoice_id         INTEGER REFERENCES invoices (id),
    amount             REAL    NOT NULL,
    payment_method     TEXT    NOT NULL,
    date               TEXT    NOT NULL,
    notes              TEXT,
    sync_status        TEXT    NOT NULL DEFAULT 'pending',
    created_at         TEXT    NOT NULL DEFAULT (datetime('now')),
    created_by_user_id INTEGER REFERENCES users (id)
);

CREATE INDEX idx_payments_customer ON payments (customer_id, date);
CREATE INDEX idx_payments_invoice  ON payments (invoice_id);

-- Maintained rather than computed on every read, for the same reason discount_amount is
-- frozen: a list of four hundred invoices should not re-derive four hundred balances, and
-- a stored figure is the one the shop saw. Unlike the discount figures these are *not*
-- frozen — a later payment or credit note is supposed to move them — so every write goes
-- through one function, `Db::refresh_invoice_balance`, which rewrites all three together.
ALTER TABLE invoices ADD COLUMN amount_paid    REAL NOT NULL DEFAULT 0;
ALTER TABLE invoices ADD COLUMN amount_due     REAL NOT NULL DEFAULT 0;
-- 'unpaid' | 'partially_paid' | 'paid'. Derived from amount_due, stored so the history
-- list can filter and sort on it without recomputing.
ALTER TABLE invoices ADD COLUMN payment_status TEXT NOT NULL DEFAULT 'paid';

CREATE INDEX idx_invoices_status ON invoices (payment_status, customer_id);

-- Every invoice raised before this migration was marked with a payment type at the
-- counter, which was as close as the old model came to saying "this was settled". They
-- are backfilled as paid in full rather than as a sudden pile of debt, because that is
-- what the shop believed at the time — and inventing a payments row for each of them
-- would be inventing a receipt that was never issued.
UPDATE invoices
   SET amount_paid = grand_total,
       amount_due = 0,
       payment_status = 'paid';

-- How much a customer may owe at once. NULL means nobody has set one, which is not the
-- same as zero: a shop that has never thought about credit limits should not have every
-- credit sale refused.
ALTER TABLE customers ADD COLUMN credit_limit REAL;
