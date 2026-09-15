# Office Console — Stage 17 (three usability fixes)

Three problems found by actually billing on the console rather than by reading the code.
No redesign: each of these is the smallest change that makes the screen behave the way
somebody at a counter already expects it to.

## 1. The customer attach flow was invisible

The mobile search sat unlabelled in the page header, to the right of the title, where it
read as a global search box. People did not find it, added their items, pressed **Print &
Lock**, and got *"Attach a customer first."* — a message that names a requirement and then
leaves you hunting for where to satisfy it.

**Customer is now a titled section at the top of the billing card**, above the item rows,
where a bill actually starts. It shows exactly one of three things:

- the **Mobile Number** box with a **Search** button, while nobody is attached;
- the attached customer, as a persistent **"Billing to: … · GSTIN … · place of supply …"**
  line with an **×** to detach and search again;
- **"No customer with 9876500011 yet."** with an inline **Add New Customer** when the
  number matches nobody.

A miss is not an error. It is the second most common thing that happens at a counter, so
it gets an answer on the spot: the form opens below with the number already filled in, and
saving attaches the customer to the bill immediately rather than dropping you back at a
search box for the number you just typed.

**"Attach a customer first" is now a way there, not just a complaint.** It scrolls to the
Customer section, rings it briefly, focuses the mobile box, and carries a **Go to
Customer** button for a second trip.

The "Billing to" line is the one thing that survives locking: a saved invoice must still
say who it was billed to. Its **×** does not, because a saved invoice is not editable.

## 2. Password fields had no show/hide

Added to **every** password input — login, first-run setup, and Add User — as one
function, `UI.enhancePasswordFields()`, rather than five copies of the same markup. It
walks the DOM for `input[type="password"]`, marks each one so it is never wrapped twice,
and builds the eye button around it. A password field added to any future screen inherits
the toggle without anyone remembering to.

Hidden by default; the icon flips to eye-with-slash while shown; the caret position is
restored across the type switch, which browsers otherwise throw away.

## 3. One-off items, and clearing the demo data

A counter sells things that are not in the catalogue — a repair charge, a loose fitting, a
delivery. The picker now offers **+ Add "[what you typed]" as a one-off item** when nothing
matches, with Description, Rate, Tax %, Qty and Unit, and puts it straight on the bill.

### Why a one-off still gets an `items` row

`invoice_lines.item_id` and `credit_note_lines.item_id` are foreign keys, and the printed
sheet, the sync contract and credit notes all resolve a line through them. Making the
column nullable to accommodate ad-hoc lines would ripple through every one of those for no
gain. Instead a one-off gets a real row carrying `custom = 1`, and that flag keeps it out
of `search_item`, `list_items` and `count_items` — so it is billable, creditable and
reprintable, but the catalogue a shop maintains never grows by one row a day.

The generated code (`ONE-OFF-<uuid>`) is a key, not a label. The billing card, the history
detail and the reprint all render a **custom** tag in the Item Code column instead, and the
printed sheet an em dash; the description is what the line is.

`create_custom_item` is open to any signed-in user — a cashier has to be able to bill a
repair charge — unlike `save_item`, which edits the catalogue.

### Clear Demo Data

Owner-only, under Settings. It removes the sample customers and stock the console shipped
with, **matched by their seeded item codes and mobile numbers**, so a customer the shop
entered itself is never in scope even if nothing has been billed to them yet.

A seeded row goes only if nothing has ever been billed against it:

```sql
DELETE FROM items
 WHERE item_code = ?1
   AND id NOT IN (SELECT item_id FROM invoice_lines)
   AND id NOT IN (SELECT item_id FROM credit_note_lines)
```

An invoice is a document that has left the building, so anything it points at has to stay
readable for as long as the invoice does. The result reports what it kept as well as what
it removed — *"Removed 6 item(s) and 4 customer(s). Kept 2 that have been billed."* — so
the exception is visible rather than looking like a partial failure.

Nothing is queued for sync. These rows were never business data; telling the back office
to delete them would be telling it about records it should never have been sent.

Refused while a transaction is in progress on the billing screen, since that bill holds
item ids this would delete and the save would fail on a foreign key with nothing useful
to say.

## Verified end to end

Driven through the real app under Xvfb, on a fresh database:

1. Searched **9876500011**, which matched nobody, used the inline **Add New Customer**,
   and the bill came back reading *"Billing to: Vel Murugan Hardware · GSTIN
   33ABCDE1234F1Z5 · 9876500011 · place of supply TN"*.
2. Added *"Shutter spring replacement"* as a one-off at ₹1,450 × 3 at 18%. Core priced it
   at 4,350 + 391.50 + 391.50 = **₹5,133.00**; the line carried the **custom** tag and
   Inventory still listed exactly the 7 seeded items.
3. Detached the customer and pressed **Print & Lock**: *"Attach a customer first."* with a
   **Go to Customer** button, and the Customer section focused. Re-attached, saved
   **RI-2026-0001**, and the reprint rendered the one-off with an em dash for its code.
4. Toggled the password on the login screen, the first-run screen and Add User.
5. Billed **RI-2026-0002** to a seeded customer with a seeded item, then ran **Clear Demo
   Data**: 6 items and 4 customers removed, TMT-12MM and Sri Balaji Traders kept, both
   invoices intact in History, and the one-off invoice still reading in full.

Checked in light and dark. 136 tests pass; clippy is clean.

## Also in this change

`describe_wait` had an unreachable branch — `(seconds + 59) / 60` can never be 1 once
`seconds <= 60` has already returned — which left a test asserting a string the function
could not produce. The branch is gone and the test now asserts the ceiling behaviour that
was always intended: over-stating a wait by a few seconds costs nothing, under-stating it
is the app telling somebody to come back at a moment when it will still refuse them.

## Not done

The one-off form does not remember anything: billing the same repair charge twice means
typing it twice. Live search as you type was considered for the mobile box and not done —
a partial mobile number matching the wrong customer mid-type is worse than pressing Enter.
Clear Demo Data cannot be undone and has no dry run beyond the confirmation dialog.
