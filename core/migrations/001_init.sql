-- 001_init: customers, items, invoices, invoice_lines, sync_queue.

CREATE TABLE customers (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT    NOT NULL,
    gstin            TEXT,
    place_of_supply  TEXT    NOT NULL,
    mobile           TEXT    NOT NULL UNIQUE,
    created_at       TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_customers_mobile ON customers (mobile);

CREATE TABLE items (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    item_code    TEXT    NOT NULL UNIQUE,
    description  TEXT    NOT NULL,
    rate         REAL    NOT NULL,
    tax_rate     REAL    NOT NULL,
    uom          TEXT    NOT NULL,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_items_item_code ON items (item_code);

CREATE TABLE invoices (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    invoice_no   TEXT    NOT NULL UNIQUE,
    date         TEXT    NOT NULL,
    customer_id  INTEGER NOT NULL REFERENCES customers (id),
    subtotal     REAL    NOT NULL DEFAULT 0,
    cgst         REAL    NOT NULL DEFAULT 0,
    sgst         REAL    NOT NULL DEFAULT 0,
    igst         REAL    NOT NULL DEFAULT 0,
    grand_total  REAL    NOT NULL DEFAULT 0,
    payment_type TEXT    NOT NULL,
    sync_status  TEXT    NOT NULL DEFAULT 'pending',
    created_at   TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_invoices_date ON invoices (date);
CREATE INDEX idx_invoices_customer_id ON invoices (customer_id);

CREATE TABLE invoice_lines (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    invoice_id  INTEGER NOT NULL REFERENCES invoices (id) ON DELETE CASCADE,
    item_id     INTEGER NOT NULL REFERENCES items (id),
    qty         REAL    NOT NULL,
    rate        REAL    NOT NULL,
    tax_rate    REAL    NOT NULL,
    line_total  REAL    NOT NULL
);

CREATE INDEX idx_invoice_lines_invoice_id ON invoice_lines (invoice_id);

-- Every local write lands here so a later stage can drain it to the back office.
-- Nothing consumes this table yet: queue it, don't send it.
CREATE TABLE sync_queue (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    table_name   TEXT    NOT NULL,
    row_id       INTEGER NOT NULL,
    op           TEXT    NOT NULL CHECK (op IN ('insert', 'update')),
    payload_json TEXT    NOT NULL,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    synced_at    TEXT
);

CREATE INDEX idx_sync_queue_unsynced ON sync_queue (synced_at) WHERE synced_at IS NULL;
