# Office Console — Stage 18 (price lists)

An item had exactly one rate, on `items.rate`. A shop that sells the same bag of cement at
one price over the counter and another to a builder had no way to say so, and the cashier
typed the difference in by hand — which is not a price list, it is a habit. Discounts, the
ledger and the reports all sit on top of pricing, so pricing gets to be right first.

## The rule

**A rate is core's to decide, not the screen's.** The billing card no longer sends a rate
at all: it sends the item and the quantity, and core resolves the price from the customer
the *database* has on file. The totals the screen displayed still travel, as `expected`,
and a disagreement refuses the save. Before this change that guard could only confirm core
agreed with itself; now it catches the case that matters — a bill totalled at retail for a
customer who is on wholesale.

## The shape

```
price_lists    id, name (unique, case-insensitive), is_default, created_at
item_prices    item_id, price_list_id, rate        UNIQUE (item_id, price_list_id)
customers      + price_list_id  (nullable)
```

Resolution is three steps, and `Db::resolve_rate` is the only thing billing asks:

1. the customer's `price_list_id`, or the default list when it is null;
2. `item_prices` for that item on that list;
3. failing that, the item's base `rate`.

**`items.rate` stays, as the base price and the fallback.** That is what lets a shop create
a Wholesale list and set rates for the twenty items it actually discounts rather than all
nine hundred. It is also what a one-off item uses — a line typed onto a single bill has no
business appearing in a price list, so `create_custom_item` writes no price rows and a
one-off falls back on every list.

**Exactly one default, enforced by the database.** A partial unique index
(`ON price_lists (is_default) WHERE is_default = 1`) makes two defaults unreachable, and
the only way to move the flag is `set_default_price_list`, which clears the old one and
sets the new one in one transaction — in that order, because the index would reject the
other. There is no operation that clears the flag without naming a replacement, which is
what keeps "exactly one" from decaying into "at most one".

**`customers.price_list_id` is nullable rather than pre-filled with today's default.** A
customer on "no list" follows the flag; one pinned to the id that happened to be default
when they were registered would be stranded the day the shop moves it. `upsert_customer`
preserves an existing assignment when the payload does not mention one, so re-seeding or a
sync of a stale record cannot quietly move a wholesale buyer back to retail.

## Migration

`007_price_lists.sql` creates a **Retail** list, marks it default, and copies every
existing item's rate into `item_prices` under it — so the first bill after the migration
prices exactly as the last one before it did. Custom items are excluded. `seed_demo_data`
does the same for a fresh install, so a new database and a migrated one are identical
rather than one stating its default rates and the other falling back to them.

## On screen

- **Billing** shows `Pricing: Wholesale` beside the customer. For an owner it is a
  dropdown rather than a label — the place you notice the wrong list is the place you
  should be able to fix it — and it reads the customer's own assignment, so "Default" stays
  distinguishable from "assigned to the list that happens to be default". A locked invoice
  shows the plain label: a saved bill cannot be repriced.
- **Attaching a customer re-rates what is already on the bill**, because items get added
  before the phone number does. The change announces itself: *"Prices updated for Kavi
  Constructions — Wholesale rates on 1 line(s)."* A total that changes silently is how a
  counter ends up arguing about a printed bill. Detaching, and moving the default, re-rate
  the same way.
- **The picker quotes the rate that will be billed**, resolved for the active list, with a
  small tag naming the list when the rate came from it. No tag means it fell back to the
  base rate, which is the same signal in the other direction.
- **The item form's single Rate box is now a price table**: Base rate first, then one row
  per list. A blank row has no entry at all, which is what makes it fall back rather than
  bill zero — so the boxes are empty rather than pre-filled, and saving replaces the whole
  table for that item.
- **Settings → Price Lists** (owner-only) creates, renames and re-defaults. Renaming does
  not touch rates: a list is renamed because the shop calls it something else.

Assigning a customer to a list is owner-only, like the lists themselves. What a buyer pays
is a commercial decision, not a counter one — a cashier who could move a customer onto
Wholesale mid-sale could discount any bill at will. A cashier still sees which list is
being applied, because otherwise the total on their screen is unexplainable.

## Verified end to end

Driven through the real app under Xvfb, on a fresh database:

1. Created **Wholesale** under Settings; Retail stayed default and Wholesale offered
   "Make default".
2. Set cement's Wholesale rate to **365** against its base/Retail **410** in the item
   form's price table.
3. Registered **Kavi Constructions** on Wholesale from the billing screen's customer form.
   The picker quoted **₹365.00** with a WHOLESALE tag; 100 bags billed as
   36,500 + 28% = **₹46,720.00** (RI-2026-0001).
4. Billed the same 100 bags to **Sri Balaji Traders**, left on the default: **₹410.00** a
   bag, 41,000 + 28% = **₹52,480.00** (RI-2026-0002).
5. Added cement with nobody attached — priced at the default's 410 — then attached Kavi
   Constructions. The line moved to 365, the total from ₹52,480 to ₹46,720, and the toast
   said so.
6. **TMT-12MM**, which Wholesale does not price, quoted **₹620.00** with no tag and billed
   at the base rate to a wholesale customer (RI-2026-0003).
7. Moved Sri Balaji Traders onto Wholesale from the chip dropdown and back again: the open
   bill re-rated both ways, and **RI-2026-0002 still reads 410.00 / ₹52,480.00**.

151 tests pass; clippy is clean.

## Also in this change

`check_expected_totals` no longer re-derives rates. Core hands it lines it has already
priced, through `Db::price_invoice_lines`, so there is still exactly one copy of the
pricing rules and the guard now covers every row — including the ones that defer to the
price list, which it used to skip.

The `customers` sync payload gains `price_list_id`, `#[serde(default)]` like `custom`
before it, so a row queued before this change reads as null.

**The lists themselves are not synced.** `price_lists` and `item_prices` write no
`sync_queue` rows, which means a non-null `price_list_id` on a synced customer names a row
the back office has never been sent. That is a dangling reference on the wire, and it is
recorded as such in `docs/sync-protocol.md` rather than papered over: whether pricing is
per-node or shop-wide is a decision for the Back-Office Web, which does not exist yet, and
guessing at it here would be inventing a contract nothing has agreed to.

## Not done

An invoice does not record which list it was billed from. The rate is on the line, so the
document is self-contained and a later price change cannot alter it — but "why was this
billed at 365" has to be answered from the customer's current assignment, which may have
moved since. Worth a column when the ledger lands.

Price lists cannot be deleted, only renamed and re-defaulted. There is no Customers pane:
a customer's list is set when they are registered, or from the dropdown on the billing
screen's customer chip, which means changing an existing customer's pricing means finding
them by mobile number on the billing screen. And nothing warns an owner that a list prices
nothing — an empty Wholesale list silently bills every item at its base rate, which is
correct and indistinguishable from a list somebody forgot to fill in.
