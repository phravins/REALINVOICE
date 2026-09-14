# Office Console — Stage 16 (credit notes)

Invoices were append-only with no way to fix an error. That is right about invoices and
wrong about shops: goods come back, quantities get mistyped, orders get cancelled after
the bill is printed.

## The rule

**The invoice is never edited and never deleted.** It stays exactly as it was billed,
because that is what the customer holds and what a return was filed on. A correction is a
*separate, linked document* — a credit note — that nets against it. A test asserts the
invoice row is byte-for-byte unchanged after a credit note is issued.

**Cancellation is not a special case.** It is a credit note covering every line at full
quantity. One path, one set of rules, nothing that only runs on the rare day and is
therefore never exercised.

## What was built

`credit_notes` and `credit_note_lines`, with `CN-YYYY-NNNN` running as a **separate
counter** from `RI-YYYY-NNNN` — as GST expects, and enforced: an invoice number offered to
the credit note series is rejected as the wrong document rather than continued from.

Both tables are queued for sync like every other mutation, note before lines, so the back
office never receives a line whose parent has not arrived.

## Decisions worth recording

**Totals are recomputed, never apportioned.** Crediting 3 of 7 units prices and taxes three
units through the same GST code that priced the invoice. Taking three sevenths of a stored
total is how rounding errors get into a tax return. The test invoice deliberately carries
two different tax rates — racks at 18%, cement at 28% — because a credit note that quietly
applied one rate to the whole document would pass a single-rate test.

**The credit reverses the tax that was actually charged**, derived from the invoice rather
than recomputed from the customer's current place of supply. If an address is corrected
after the sale, the refund must still reverse what was charged, not what would be charged
today.

**Lines are priced from the invoice line, not the item master.** A price change after the
sale must not change the refund.

**The ceiling is what remains, not what was billed.** A line can be credited across several
notes, and the sum can never exceed the original however the notes are ordered — proven by
returning 40 bags in four lots of ten and then being refused the forty-first.

**A reason is required**, and blank-after-trim is refused. A correction with no stated
reason is the one an auditor asks about and nobody can answer.

## The role gate

Issuing a credit note is **owner-only**, enforced in the command and not merely by hiding
the button — a cashier who could void their own sales is the shape of most till fraud.

Reading them is not gated. A cashier looking at a partly-returned invoice has to see the
credit and the net, or the figure on their screen is wrong.

Attribution comes from the session. The payload type carries no user field at all, so a
caller cannot record a reversal as somebody else's decision — the rule invoices already
follow.

## Reporting

Analytics now leads with **net earned** and keeps billed beside it. Both are true: what was
billed is what the invoices say, what was earned is what is left after corrections, and a
pane showing only one would answer a different question from the one asked.

Credit notes count in the range they were *issued* in, not pushed back to the invoice's
date — a return in October reduces October, which is the month somebody files. "What sold"
ranks on revenue after credits, because an item sold ten times and returned nine is not the
best seller. The payment mix attributes a credit to the payment type of the invoice it
reverses: money refunded on a card sale did not arrive as cash. A day that had only returns
still appears, with a negative net, rather than being dropped.

## Verified

Driven through the real UI under Xvfb, as owner then as cashier:

1. Billed RI-2026-0001 — 2 racks at 45,000 (18%) and 40 cement at 410 (28%) — ₹1,27,192.
2. Issued CN-2026-0001 for **one rack**, reason recorded. SQLite confirms the invoice row
   and both invoice lines unchanged, and the note at 45,000 + 4,050 + 4,050 = ₹53,100,
   attributed to the owner, with both rows queued for sync.
3. The detail view shows the invoice as billed, the credit note listed underneath with its
   lines, and **Net after credits ₹74,092.00**.
4. Signed in as a cashier: the credit note and net are visible, **"Issue Credit Note" is
   not**, and neither is Users.
5. Analytics: Net earned ₹74,092 ("₹1,27,192.00 billed less ₹53,100.00 credited"), taxable
   value ₹61,400, tax ₹12,692 net of credits, and "What sold" showing the rack at **1.00
   (−1.00)** rather than 2.

126 tests pass; clippy is clean.

## Also in this change

The login lockout window dropped from 15 minutes to **one** — long enough that guessing at
bcrypt speed is pointless, short enough that a mistyped password does not cost a counter
its next customer. The screen now quotes the window from core's constant instead of writing
it out, so the two cannot drift apart again.

## Not done

No credit note on the printed sheet — it is an on-screen and stored record only, and a
customer asking for a printed credit note cannot be given one yet. Nothing prevents
crediting an invoice that was never paid. And a credit note cannot itself be corrected: the
only remedy for a wrong one is another one, which is defensible but undocumented on screen.
