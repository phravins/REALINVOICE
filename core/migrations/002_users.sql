-- 002_users: local sign-in and invoice attribution.
--
-- Credentials live only on this machine. There is no cloud backend yet, and deliberately
-- no sync_queue row is written for a user: replicating password hashes off the node is a
-- decision for whoever builds the sync worker, not something to start doing by default.

CREATE TABLE users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT    NOT NULL,
    display_name  TEXT    NOT NULL,
    role          TEXT    NOT NULL CHECK (role IN ('owner', 'cashier')),
    created_at    TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX idx_users_username ON users (username);

-- Who billed it. Nullable: invoices raised before this migration have no signed-in user
-- to attribute them to, and inventing one would be a lie.
ALTER TABLE invoices ADD COLUMN created_by_user_id INTEGER REFERENCES users (id);

CREATE INDEX idx_invoices_created_by ON invoices (created_by_user_id);
