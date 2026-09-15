-- 006_custom_items: one-off items, billed without joining the catalogue.
--
-- A counter often has to bill something that is not in the price list — a delivery
-- charge, a one-time fitting, an item nobody has set up yet. Before this, the only way
-- was to leave the bill, add it to Inventory, and come back, which is why it did not
-- happen and things got billed under the wrong line instead.
--
-- Such an item still gets a row here rather than living loose on the invoice line:
-- `invoice_lines.item_id` is a foreign key, and so is `credit_note_lines.item_id`.
-- Making those nullable would ripple through the printed sheet, the sync contract and
-- the credit path for no gain. The flag is what keeps it out of the catalogue: a custom
-- item is invisible to the Inventory list and to the billing picker, so the price list
-- a shop maintains stays the price list it maintains.
ALTER TABLE items ADD COLUMN custom INTEGER NOT NULL DEFAULT 0;

-- The catalogue reads are all "the non-custom ones", so index that way round.
CREATE INDEX idx_items_custom ON items (custom, item_code);
