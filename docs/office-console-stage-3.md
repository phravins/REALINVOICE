# Office Console — Stage 3

Print & Lock now saves. The invoice gets a real number, the screen locks, and a New
Transaction button resets the card for the next customer.

## The save

`create_invoice` hands the assembled payload to core, which does everything in **one
transaction**: allocate the next number in the financial year, insert the invoice, insert
each line, and queue a `sync_queue` row for the invoice and for every line. Either the
whole thing lands or none of it does.

The command returns more than the invoice, because the locked screen needs it: the stored
lines, the buyer, and how many rows are now waiting in `sync_queue` (shown in the status
bar, so the queuing from core's first stage stays visible rather than assumed).

### The number is allocated at save, never before

Stage 2 left a race: core read `MAX(invoice_no)` *before* opening its transaction, so two
consoles saving at the same moment could read the same highest number and then both try to
insert it. Fixed here — the read now happens inside a transaction opened with
`TransactionBehavior::Immediate`, which takes the write lock up front, so the number is
read and used under the same lock that inserts it. `PRAGMA busy_timeout = 5000` was added
alongside it so a second writer waits its turn instead of failing the sale.

Nothing previews the number client-side. It first appears on screen when the save returns
it.

A test races eight connections against one file through a barrier and asserts they come
back with `RI-2026-0001` … `RI-2026-0008` — distinct, contiguous, one queued sync row
each.

### The save refuses to disagree with the screen

The panel's totals go along with the payload as `expected`. When every row carries its own
rate and tax rate — which the billing card always sends — the command re-prices them
through core's GST module and refuses the save if anything differs by more than half a
paisa. An invoice can therefore never be stored under figures the operator did not see.

Rows that defer to the item master are priced inside core during the save; re-deriving
that here would mean a second copy of core's pricing rules, so those rows skip the check
rather than get a guess.

## Locking

On success the card goes read-only: Qty boxes, row removal, Add Item, customer search and
the payment selector are all disabled, the border turns green, the invoice number appears
in the header beside the title, and the confirmation reads **Saved — Invoice
#RI-2026-0001**. Print & Lock is replaced by **New Transaction**, which clears customer,
rows, totals and number, and returns the card to a fresh empty state with the mobile box
focused.

A saved invoice is a printed document. Nothing on it may still look editable, or an
operator will "correct" a row that is already in the books and on a customer's copy.

## Failure

A failed save does not lock. The error appears inline next to Print & Lock and in the
status bar, and the form stays exactly as it was so the transaction can be retried with
nothing re-keyed. Because core rolls the whole transaction back, a failure also burns no
invoice number: the next successful save takes the one that failed.

## Verified end to end

Run under Xvfb against a fresh config dir:

1. Billed the stage-2 example (1 × 42U rack, 5 × licence) to Ishta Capital Investments.
   Before saving, the header showed no number.
2. F5 → header shows `RI-2026-0001`, confirmation **Saved — Invoice #RI-2026-0001**, card
   locked green, status bar `Saved RI-2026-0001 · ₹1,23,900.00 · 2 line(s) · 15 row(s)
   queued for sync.`
3. Typing into a Qty box, clicking a row's remove button and clicking Add Item on the
   locked card all did nothing — rows, quantities and totals unchanged.
4. Opened `db.sqlite` from a separate process: one `invoices` row (1,05,000.00 / 9,450.00 /
   9,450.00 / 0 / 1,23,900.00, `sync_status` `pending`), two `invoice_lines` rows
   (45,000.00 and 60,000.00), and three unsynced `sync_queue` rows — one `invoices`
   insert, two `invoice_lines` inserts.
5. New Transaction cleared everything back to an empty editable card.
6. Billed a second, different invoice (20 × cement to Kaveri Hardware) → `RI-2026-0002`,
   ₹10,496.00. Both invoices in the file with distinct numbers; the first kept its own
   lines and figures.
7. Error path: attached a customer, added a row, deleted that customer row from the file
   from another process, then hit F5. Inline error **customer 3 no longer exists**, no
   lock, no number, Print & Lock still offered. Editing the Qty afterwards re-priced live,
   and the database still held exactly the two earlier invoices.

> **Extended by stage 4.** The history pane arrived, and Print & Lock gained a Print
> preview that shares its layout with Reprint. See
> [office-console-stage-4.md](office-console-stage-4.md).

## Not in this stage

- The invoice history / list pane — stage 4. Persistence was confirmed by reading
  `db.sqlite` directly.
- Actual printing. "Print & Lock" saves and locks; no print dialog is opened yet.
- Anything that drains `sync_queue`.
- Editing or voiding a saved invoice. There is no way back from a lock except New
  Transaction, which starts a fresh one.
