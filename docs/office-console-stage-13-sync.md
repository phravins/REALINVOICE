# Office Console — Stage 13 (outbound sync)

`sync_queue` has been filled since the original core build and drained by nothing. This
stage builds the worker that drains it.

## The rule everything follows

**Billing never waits for the network.** The worker owns no part of the write path. It
takes the shared connection only long enough to read a batch or mark one sent, and holds
it across no `.await` — a back office that is down, slow or wrong is a queue that grows,
never a till that stops.

That is not a claim; it is what was tested. An invoice was billed against an endpoint
returning 500 and against one refusing connections, and both saved normally.

## What was built

`core/src/sync.rs`, behind a default-on `sync` feature so the Logistics Desk TUI — which
has no business linking a TLS stack — can compile without it. The desktop spawns it on
Tauri's own runtime: core hands back a future rather than calling `tokio::spawn` itself,
because a library that reaches for a runtime only works inside a context it cannot see.

Three new commands — `sync_status`, `sync_now`, `set_sync_endpoint` (owner-only; where a
shop's invoices are sent is not a cashier's decision) — plus the queue primitives core was
missing: `next_sync_batch`, `pending_sync_count`, `mark_synced`.

## Two things found on the way

**Invoice lines were queued before their invoice.** `create_invoice` called `insert_line`,
which enqueues, before enqueueing the invoice itself. Harmless for as long as nothing
consumed the queue, which is exactly why it survived this long — but the worker sends in
queue order, so the back office would have received lines referencing an invoice that had
not arrived. The enqueue now happens before the line loop, inside the same transaction, so
nothing about the local write changed.

**An idle, fully-synced till read "Offline".** The first cut skipped the request when the
queue was empty. With nothing to send, the last success aged past the badge's window and
a shop that was perfectly healthy went amber. Claiming a success that never happened would
have been worse, so the worker now sends an empty batch as a heartbeat and actually
checks. The far end answers 200 to `rows: []`, and gets a liveness signal for a till that
simply has not billed anything yet today.

## The badge

Green **Connected** when the last accepted batch landed within twice the poll interval;
grey **Not linked** when no address is set, because a fresh install is waiting to be
configured rather than failing; amber **Offline — N pending** otherwise, carrying the
count so nobody has to guess. The verdict is computed in Rust, not JavaScript, so every
runtime that shows this agrees on what connected means. Clicking the badge opens the Sync
page that explains it.

## On the wire

Documented in `docs/sync-protocol.md` and pinned by tests.

**No Ecto schema was matched, because there is none.** The task asked to confirm the JSON
shape matches what stage 2's schemas expect field-for-field; stage 2 does not exist — there
is no Elixir anywhere in this repository. That document therefore records what the desktop
actually sends, for stage 2 to be built against. If the schemas later differ, this is the
side already running on real tills.

The token is a placeholder, the same string on every node, and authenticates nothing. It
is labelled as such in the code, the protocol document and this note, because the one way
it could do harm is being mistaken for a security boundary.

## Verified

Against a stand-in endpoint (`/tmp/mockcloud/server.py` — **not** the real back office,
which does not exist) driven through the real UI under Xvfb:

1. Fresh install, no address: badge reads **Not linked**, Sync page shows 12 queued.
2. Unreachable address: badge **Offline — 12 pending**, "could not reach the back office",
   "attempt 2, in about 10s". An invoice billed here saved normally — RI-2026-0001,
   ₹53,100, 14 rows queued.
3. Address pointed at the stand-in: badge flips to **Connected**, queue drains to zero,
   14 rows received in dependency order — customers, items, invoice, then its line — with
   `Bearer` and `x-realinvoice-node` headers, full records as JSON objects with snake_case
   keys.
4. Idle for 25s with nothing queued: stays **Connected** (the heartbeat fix).
5. Endpoint switched to 500: another invoice billed fine, badge **Offline — 2 pending**,
   and SQLite confirmed nothing marked sent across repeated rejections.
6. Endpoint switched back: backlog drained on its own, 16 rows upstream, 0 pending, both
   invoices present, no duplicates.

102 tests pass; clippy is clean.

## Not done

No inbound sync, no conflict resolution, no per-node tokens, no TLS pinning, and no
deletion of rows once acknowledged — `sync_queue` grows forever and will want pruning
before a busy counter has run for a year.
