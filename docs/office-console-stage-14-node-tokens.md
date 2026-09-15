# Office Console — Stage 14 (per-node tokens)

The sync worker shipped with a placeholder credential: one string, the same on every node,
authenticating nothing. This replaces it with a token issued to this till, and teaches the
worker what to do when the back office refuses one.

## What changed

**The placeholder is gone from the codebase**, not deprecated — there is no default, no
fallback and no constant to fall back to. `SyncConfig::load` reads the stored token or
gets an empty string, and an empty token means the worker does not send at all.

That last part is deliberate. Sending an empty bearer would produce a 401 from a node
that has simply never been registered, and the worker would then disable itself over a
situation that is not an error. **"Not registered" and "rejected" are different states**,
they read differently on the badge, and the worker treats them differently: one idles,
the other stops.

**The token is entered once, in Settings › Sync**, and stored in core's `settings` table —
this node's own SQLite file, outside the repository. It is never written to source, never
to a committable config file, and **never sent back to the frontend**. `sync_status`
carries `token_set` and a `token_hint` of the last four characters, because anything that
reaches JavaScript can be read out of the page. A test asserts the serialised status does
not contain the token.

The entry box is cleared the moment a token is saved. Leaving it on screen would leave a
credential sitting in a text box at a counter people walk past.

## 401 stops the worker

A revoked token does not become valid by being sent again, so a 401 is not a retry:

- the worker stops attempting — verified as **exactly one** 401 at the endpoint, still one
  after three further poll intervals
- the badge reads **"Sync disabled — invalid token"** in red, which is distinct from
  amber "Offline": the back office answered, and what it said was no
- the Sync screen raises a prompt saying nothing has been lost and asking for a new token
- the queue is untouched, so the backlog goes as soon as a working token is entered

It resumes only on a human action — saving a token, or pressing Sync Now. Both are a
person saying "try this", which is the one thing that should lift a self-imposed stop; the
alternative is a till that has to be restarted after the back office re-issues.

One row was fixed after seeing it on screen: the Sync page said *"Retrying: attempt 2, in
about 10s"* while the worker was deliberately not retrying. It now says **"Stopped —
waiting for a working token"**, because the first version would have left somebody waiting
for a recovery that was never coming.

## Local-first, still

Billing was tested at every state: no token, valid token, revoked token. An invoice saves
and queues identically in all three. With no token the worker makes no request at all, and
with a revoked one it makes none after the first refusal — in both cases the till bills
exactly as fast as it does offline.

## Verified

Against a stand-in registry (`/tmp/mockcloud/server.py`) implementing register, revoke,
`last_seen_at` and 401, driven through the real UI under Xvfb:

1. Address set, no token: badge **Not registered**; an invoice billed here saved
   (RI-2026-0001, ₹53,100), 14 rows queued, and **no request was made**.
2. Node registered, token pasted: badge **Connected**, token shown as `••••02db`, the box
   cleared, 14 rows delivered, `last_seen_at` set on the cloud side.
3. Second invoice billed: synced within a poll, cloud row count 14 → 16, `last_seen_at`
   advanced.
4. Node revoked, third invoice billed: saved normally (RI-2026-0003, ₹731.60), badge
   **Sync disabled — invalid token**, exactly one 401 and no further attempts.
5. Restarted the app: the rejection is re-detected rather than forgotten.
6. New token issued and pasted: badge back to **Connected**, hint now `••••6e26`, the
   backlog drained to zero.

Checked afterwards that no real token appears in any git-tracked file — the only
`rin_live_` strings in the repository are fabricated fixtures in the tests that assert
masking.

105 tests pass; clippy is clean.

## What this is not

**There is still no Back-Office Web.** Neither `phravins/realinvoice` nor
`phravins/QuantumBilling` contains a node registry, a registration screen, an admin revoke
UI or a `last_seen_at` column — QuantumBilling is a separate product (clients, e-invoice,
e-way bills) and is unchanged since the design system was ported from it. The task
described registering and revoking through that UI; what was exercised is a stand-in
implementing those semantics, so the desktop half is genuinely proven and the cloud half
remains unwritten. `docs/sync-protocol.md` now specifies the 401 contract for whoever
builds it.

The token still travels as a bearer over whatever scheme the address uses. Over `http://`
that is plaintext on the wire; TLS is the deployment's job and pinning is not done.
