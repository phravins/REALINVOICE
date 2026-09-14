-- 005_credit_notes: the correction path.
--
-- An invoice is never edited and never deleted. It stays exactly as it was billed,
-- because that is what was handed to the customer and what a GST return was filed on.
-- A mistake is corrected by issuing a *separate, linked* document that nets against it.
--
-- Full cancellation is not a special case: it is a credit note covering every line at
-- full quantity. One path, one set of rules, nothing that only runs on the rare day.

CREATE TABLE credit_notes (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Its own series, CN-YYYY-NNNN, counted separately from invoices.
    credit_note_no     TEXT    NOT NULL UNIQUE,
    original_invoice_id INTEGER NOT NULL REFERENCES invoices (id),
    date               TEXT    NOT NULL,
    -- Required, deliberately. A correction with no stated reason is the one an auditor
    -- asks about and nobody can answer, so there is no path that writes a blank here.
    reason             TEXT    NOT NULL,
    -- The credited amount, recomputed from the credited lines by the same GST code that
    -- priced the invoice — never a fraction of the original applied by proportion.
    subtotal           REAL    NOT NULL DEFAULT 0,
    cgst               REAL    NOT NULL DEFAULT 0,
    sgst               REAL    NOT NULL DEFAULT 0,
    igst               REAL    NOT NULL DEFAULT 0,
    grand_total        REAL    NOT NULL DEFAULT 0,
    sync_status        TEXT    NOT NULL DEFAULT 'pending',
    created_at         TEXT    NOT NULL DEFAULT (datetime('now')),
    created_by_user_id INTEGER REFERENCES users (id)
);

CREATE INDEX idx_credit_notes_invoice ON credit_notes (original_invoice_id);
CREATE INDEX idx_credit_notes_date ON credit_notes (date);

CREATE TABLE credit_note_lines (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    credit_note_id INTEGER NOT NULL REFERENCES credit_notes (id) ON DELETE CASCADE,
    -- Which line of the original invoice this credits. Not just the item: an invoice can
    -- carry the same item twice at different rates, and a credit has to say which one.
    invoice_line_id INTEGER NOT NULL REFERENCES invoice_lines (id),
    item_id        INTEGER NOT NULL REFERENCES items (id),
    -- Full or partial. Never more than the line carried, and never more than what is left
    -- uncredited after earlier notes against the same invoice.
    qty            REAL    NOT NULL,
    -- Copied from the invoice line, not re-read from the item master: crediting a sale
    -- has to use the price it was sold at, whatever the catalogue says today.
    rate           REAL    NOT NULL,
    tax_rate       REAL    NOT NULL,
    line_total     REAL    NOT NULL
);

CREATE INDEX idx_credit_note_lines_note ON credit_note_lines (credit_note_id);
CREATE INDEX idx_credit_note_lines_invoice_line ON credit_note_lines (invoice_line_id);
