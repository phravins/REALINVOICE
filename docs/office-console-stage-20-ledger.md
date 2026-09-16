# Office Console — Stage 20 (customer ledger)

`payment_type` was a label. It said "cash" because somebody picked cash at the counter,
not because any money had changed hands, and there was nowhere at all to record a sale on
account or a bill settled in two instalments. This stage makes the distinction real.

## The rule

**An invoice records what was billed. A payment records what was received.** They are
separate rows because they are separate events — a payment can arrive weeks later, in
parts, against no particular bill, from a customer clearing a running tab.

```
payments       id, customer_id, invoice_id (nullable), amount, payment_method,
               date, notes, created_by_user_id
invoices     + amount_paid, amount_due, payment_status
customers    + credit_limit (nullable)
```

`amount_due = grand_total − amount_paid − credit notes`, never below zero.

**Every write that can move a balance goes through one function**,
`Db::refresh_invoice_balance`, inside the same transaction: recording a payment, issuing a
credit note, appending a line. Three columns that could disagree with each other are three
columns that eventually will, so nothing sets any of them directly.

The one exception is the moment an invoice is created, where the answer is already known —
a brand-new invoice has no credit notes and at most the one payment beside it — so the row
is written complete. That also keeps a save to a single `sync_queue` entry instead of an
insert chased by an update correcting it.

## What the payment selector now means

Cash, UPI and Card mean the money arrived: the save writes a matching `payments` row for
the full grand total, and the invoice is `paid`. **Credit / On Account** means it did not:
no payments row, `amount_paid = 0`, `payment_status = unpaid`, and the bill goes onto the
customer's account.

A zero-total invoice — everything discounted away — gets no payments row either. Nothing
was received, and nothing is owed.

## Overpayment

A payment aimed at an invoice that is larger than that invoice still owes becomes **two
rows**: one settling the bill, one left on the account. Refusing it would send somebody
away holding cash the shop would not take; keeping the difference quietly would lose money
that was handed over. Two rows because each then means exactly one thing, and both show in
the ledger. The screen says so too, rather than leaving anyone to work out where the rest
went.

A customer's balance is `Σ invoice.amount_due − Σ unallocated payments`, so it can go
negative — the shop holding their money is a real state, and the ledger labels it "In
credit" rather than showing a minus sign.

## The ledger

Invoices, payments and credit notes interleaved by date with the balance after each, built
in Rust rather than SQL because the three kinds live in three tables and the order matters
more than the query does. Same-date entries run **invoice, then credit note, then
payment** — what was billed before what came off it — so a bill settled the day it was
raised reads the way it happened instead of showing a payment against a balance that does
not exist yet.

A credit note reduces `amount_due` exactly as a payment does, and is counted separately: it
is not money received, it is a bill that should have been smaller. The ledger labels the
three kinds distinctly and the totals strip reports Billed, Received and Credited apart.

## Customers, promoted to a screen

Customers were only reachable inline while billing. They now have a list — name, mobile,
invoice count, last billed, and **Outstanding**, sortable most-owed-first, which is the
view a shop owner actually opens — and a per-customer ledger page with the balance as the
headline figure, Record Payment, and an owner-only credit limit box.

The list's outstanding figure is computed in SQL rather than by asking per customer, so a
shop with two thousand accounts can sort by it; a test asserts that figure equals what each
customer's own ledger reports, which is what stops the list and the detail page telling two
different stories.

## Credit limits

`customers.credit_limit` is nullable, and **`None` means no limit, not a limit of zero** —
a shop that has never thought about credit limits should not find every account sale
refused. A cashier whose credit sale would take a customer past their limit needs an
owner's username and password, using the same `OwnerApproval` path as the discount gate
from stage 19. One approval covers both gates: an owner standing at the till to sign off a
sale signs off whatever about it needed signing.

Only credit sales are checked. A bill paid at the counter adds nothing to what anyone owes,
however large. The check runs in Rust inside `create_invoice`, so skipping the prompt with
a hand-made payload is refused just the same, and a cashier cannot approve themselves.

## Migration

`009_ledger.sql` backfills every existing invoice as **paid in full** — that is what the
shop believed at the time, and a sudden pile of historical debt would be a lie. It does not
invent a `payments` row for any of them: marking a bill settled is not the same as issuing
a receipt nobody ever wrote. A test builds a real database at the 008 schema, puts an
invoice in it, opens it with the new code and asserts exactly that.

`schema::migrate_through` was added for that test: a backfill only ever run against a
fresh, empty database has not been tested at all.

## Verified end to end

Driven through the real app under Xvfb:

1. Billed ₹5,248.00 on **Credit / On Account** — invoice unpaid, full amount due, and an
   amber "Outstanding: ₹5,248.00" appeared beside the customer's name on the billing card.
2. Recorded ₹2,000 against it: outstanding fell to ₹3,248.00, History showed **PARTIALLY
   PAID · 3,248.00 due**.
3. Recorded ₹3,248: balance ₹0.00, "settled up", the invoice **paid**.
4. Billed ₹14,632.00 on credit and issued a credit note for five rods (₹3,658.00). The
   ledger read, in order: +5,248.00 → 5,248.00; +14,632.00 → 19,880.00; −3,658.00 →
   16,222.00; −2,000.00 → 14,222.00; −3,248.00 → **10,974.00**.
5. Set a ₹12,000 credit limit, signed in as a cashier, and tried a ₹5,248 credit sale:
   *"This sale would take Sri Balaji Traders to ₹16,222.00 against a ₹12,000.00 credit
   limit."* Cancelling saved nothing; the owner's password saved it.
6. Sorted Customers by Outstanding: ₹16,222.00 at the top, matching that customer's ledger
   to the paisa.

189 tests pass; clippy is clean.

## Also in this change

Invoice history and the invoice detail now show payment status — a pill in the list, "Unpaid
· ₹14,632.00 due" in the detail header — and a locked credit sale's card reads
"CREDIT · ₹5,248.00 DUE" rather than just naming the payment type. "Paid" gets no pill: it
is the ordinary case, and a column of green ticks buries the two rows somebody is looking
for.

`add_line_item` no longer queues the invoice twice. It used to write its own update and
then the balance refresh wrote another; now the refresh queues once, carrying both the new
totals and the new balance, so the back office never sees a row where they disagree.

## Not done

Payments cannot be edited or reversed — a mistyped amount has to be corrected by recording
its opposite, which the schema does not allow (amounts must be positive). That is the next
obvious gap and it wants a deliberate design rather than a nullable flag.

Nothing ages the balance: there is no 30/60/90 view, no statement to send, and no reminder
that an account has been open for two months. `payments` is queued for sync but the far end
has never been told the table exists, like `price_lists` before it — recorded in
`docs/sync-protocol.md`.

Allocation is one payment to at most one invoice, which is what the schema asks for. A
single cheque settling three bills has to be entered as three payments, or as one on
account. Real double-entry would want a `payment_allocations` table; this is the shape the
stage specified and it is honest about what it can express.
