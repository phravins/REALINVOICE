# The sync protocol

What the Office Console POSTs, and what the back office has to answer.

> **This contract is not derived from an Ecto schema.** The Back-Office Web (stage 2) does
> not exist yet — there is no Elixir in this repository — so there was nothing to match
> field-for-field against. What follows is the shape the desktop *actually sends*, pinned
> by tests in `core/tests/invoice_flow.rs`. Stage 2 should be built against this document;
> if its schemas end up differing, this is the side that is already in production on real
> tills, and the migration cost falls on whichever side moves.

## The request

```
POST <configured endpoint>
authorization: Bearer <token>
x-realinvoice-node: POS-01
content-type: application/json
```

The token is a **placeholder** — `realinvoice-dev-token`, the same on every node. It
authenticates nothing. Real per-node issuance is a later hardening stage, and until then
the endpoint must not treat this header as proof of anything.

## The body

```json
{
  "node_id": "POS-01",
  "sent_at": "2026-09-13T16:40:37+05:30",
  "rows": [
    {
      "id": 13,
      "table_name": "invoices",
      "row_id": 1,
      "op": "insert",
      "recorded_at": "2026-09-13 16:39:48",
      "payload": { "...": "the whole record" }
    }
  ]
}
```

| Field | Meaning |
| --- | --- |
| `node_id` | Which till. Also in the header. |
| `sent_at` | When this request was built, ISO 8601 with offset. |
| `rows[].id` | The queue row's id **on that node**. Unique per node, never reused. |
| `rows[].table_name` | `customers`, `items`, `invoices` or `invoice_lines`. |
| `rows[].row_id` | The record's primary key on that node. |
| `rows[].op` | `insert` or `update`. |
| `rows[].recorded_at` | When the write happened locally, `YYYY-MM-DD HH:MM:SS`, local time. |
| `rows[].payload` | The full record as a JSON **object**, keys matching the column names. |

### Two properties the far end can rely on

**Rows arrive in dependency order.** They are sent oldest-queued first, and a customer is
queued before the invoice that references it, an invoice before its lines. A batch never
contains a row whose parent has not already been sent or is not earlier in the same batch.

**`(node_id, id)` is the idempotency key.** A batch that fails is retried as the same
batch with the same ids. The endpoint must treat a repeat as a no-op, not as a duplicate
record — the till has no way to know whether a request that timed out was applied.

### An empty `rows` array is a heartbeat

When a till has nothing queued it still POSTs, with `rows: []`. Answer it 200. It is how
an idle till knows it can still reach the back office, and it is what keeps the cashier's
badge honest rather than flipping to "Offline" on a shop that is simply up to date.

## Payloads

Keys are the column names. Money is a JSON number in rupees; `id` fields are integers.

```jsonc
// customers
{ "id": 1, "name": "Sri Balaji Traders", "mobile": "9840012345",
  "gstin": "33AABCS1429B1ZP",        // null when unregistered
  "place_of_supply": "TN" }

// items
{ "id": 7, "item_code": "SAND-M-UNIT", "description": "M-Sand per unit",
  "rate": 4800.0, "tax_rate": 5.0, "uom": "UNIT" }

// invoices
{ "id": 1, "invoice_no": "RI-2026-0001", "date": "2026-09-13", "customer_id": 1,
  "subtotal": 45000.0, "cgst": 4050.0, "sgst": 4050.0, "igst": 0.0,
  "grand_total": 53100.0, "payment_type": "cash",
  "sync_status": "pending",           // this node's own bookkeeping; ignore it
  "created_at": "2026-09-13 16:39:48",
  "created_by_user_id": 1 }           // null on invoices raised before sign-in existed

// invoice_lines
{ "id": 1, "invoice_id": 1, "item_id": 1,
  "qty": 1.0, "rate": 45000.0, "tax_rate": 18.0, "line_total": 45000.0 }
```

`created_by_user_id` refers to a user **on that node**. Accounts are deliberately never
synced, so the back office cannot resolve it to a person yet.

Intra-state sales carry `cgst` and `sgst` with `igst` at 0; inter-state carries `igst`
with the other two at 0. Never both.

## The response

| Status | What the till does |
| --- | --- |
| any 2xx | Marks that batch sent and moves on to the next. |
| any other | Marks **nothing**. The same batch is retried, unchanged, after a backoff. |
| no answer | Same as above. |

The body of a 2xx is not read. A non-2xx must therefore never mean "I took some of it":
partial acceptance would silently lose the rows that were not taken, because the till
marks a batch all or nothing.

## Timing

| | |
| --- | --- |
| Poll interval | 10s |
| Batch size | 50 rows |
| Backoff | doubles from the poll interval, capped at 5 minutes |
| Request timeout | 20s |

Backoff starts at the poll interval rather than at zero, so one failure costs one ordinary
poll and only a run of them backs further off. A till that has been offline overnight
recovers on its own within five minutes of the link returning.
