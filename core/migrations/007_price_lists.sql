-- Wholesale vs retail pricing.
--
-- Until now an item had exactly one rate, living on `items.rate`. A shop that sells the
-- same bag of cement at one price over the counter and another to a builder had no way to
-- say so, and the cashier typed the difference in by hand — which is not a price list, it
-- is a habit.
--
-- `items.rate` deliberately stays. It is the item's base price and the fallback for any
-- list that has no entry for that item, so a shop can create a Wholesale list and set
-- rates for the twenty items it actually discounts rather than all nine hundred. It is
-- also what one-off items use, which is why the backfill below skips them: a line typed
-- onto a single bill has no business appearing in a price list.

CREATE TABLE price_lists (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    is_default  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- Exactly one default, enforced here rather than by whoever remembers to. A partial
-- unique index allows any number of zeroes and at most a single one, so the "two defaults"
-- state is unreachable even if some future caller forgets to clear the old one.
CREATE UNIQUE INDEX idx_price_lists_one_default ON price_lists (is_default)
    WHERE is_default = 1;

CREATE TABLE item_prices (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id       INTEGER NOT NULL REFERENCES items (id) ON DELETE CASCADE,
    price_list_id INTEGER NOT NULL REFERENCES price_lists (id) ON DELETE CASCADE,
    rate          REAL    NOT NULL,
    UNIQUE (item_id, price_list_id)
);

CREATE INDEX idx_item_prices_lookup ON item_prices (price_list_id, item_id);

-- NULL means "whichever list is default", rather than pinning every existing customer to
-- today's default and quietly stranding them if it changes.
ALTER TABLE customers ADD COLUMN price_list_id INTEGER REFERENCES price_lists (id);

-- The list every existing customer and item has implicitly been on all along.
INSERT INTO price_lists (name, is_default) VALUES ('Retail', 1);

-- Every existing item keeps its current price, now stated rather than assumed, so the
-- first bill after this migration prices exactly as the last one before it did.
INSERT INTO item_prices (item_id, price_list_id, rate)
SELECT id, (SELECT id FROM price_lists WHERE is_default = 1), rate
  FROM items
 WHERE custom = 0;
