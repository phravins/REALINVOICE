# Office Console — Stage 5

Local sign-in, invoice attribution, a real payment-type selector, keyboard shortcuts with
a footer bar that advertises them, and a polish pass over the billing screen.

## Sign-in

### Schema (migration `002_users`)

```sql
users(id, username UNIQUE COLLATE NOCASE, password_hash, display_name, role, created_at)
invoices.created_by_user_id  -- nullable, references users(id)
```

Passwords are hashed with **bcrypt** — one function each way, with the salt and cost
carried inside the hash string, so there is nothing for a caller to store or get wrong.
`core::auth` is the only module that sees a plaintext password.

The column is nullable on purpose: invoices raised in stages 1–4 have nobody to attribute
them to, and inventing an owner for them would be a lie.

**No `sync_queue` row is written for a user.** Every other write queues; replicating
password hashes off the machine is a decision for whoever builds the sync worker, and
starting to do it by default would make that choice silently.

### Core

| Function | Notes |
| --- | --- |
| `create_user(&NewUser, password)` | Hashes, normalises the username to lowercase, refuses duplicates and passwords under 8 characters. |
| `verify_login(username, password)` | `None` for both an unknown user and a wrong password — the screen cannot be used to find out which accounts exist. A dummy hash is verified when no user matches, so a missing username costs the same time as a wrong one. |
| `get_user` / `find_user` / `count_users` / `set_password` | The primitives a later Settings > Users screen will build on. |

`User` deliberately carries **no** `password_hash` field. It crosses into the frontend,
and a hash that never leaves the database cannot leak from a UI bug — there is a test
asserting the serialized form contains neither the hash nor the password.

### Session

One `Session { token, user }` in Tauri's app state, for the life of the process. No JWT,
no expiry: this is a single desktop process, and a till that stays signed in after hours
is the thing to avoid, not a short-lived token. Closing the app ends the session.

**The gate is enforced in Rust, not just in the UI.** Every command except `auth_status`,
`login` and `logout` calls `require_session` and fails with "Not signed in." Hiding the
shell is presentation; this is what actually stops an unauthenticated caller reading
customers or writing an invoice.

### First run

If `users` is empty, an `admin` / owner account is created with a **randomly generated**
16-character password, printed once to stdout:

```
================ RealInvoice first run ================
  A sign-in account has been created on this machine:
      username: admin
      password: 8bjdCNUNC4VZCmgX
```

Generated per installation rather than hardcoded, so no two machines ship with the same
credentials and nothing secret lives in this repository. The generator skips `0/O` and
`1/l/I` because somebody has to type it by hand.

> **Beyond the brief:** the same password is also shown once on the login screen. An app
> launched from a desktop icon has no terminal to read, and would otherwise be locked out
> of itself. It is held in memory only, never written to disk, and cleared the moment
> anyone signs in.

### Attribution

`create_invoice` fills `created_by_user_id` from the session. `NewInvoicePayload` has no
such field at all, so a caller cannot bill as somebody else — a test posts a payload with
`created_by_user_id: 99` and asserts it is dropped. The history list and detail view both
name the biller.

## Payment type

UPI / Cash / Card radios replace stage 3's placeholder `<select>`. Selecting UPI reveals a
"Scan screen code to pay via UPI" box with a **placeholder** panel — not a scannable code;
QR generation is a later stage. The selection rides into `create_invoice` and is stored.
New Transaction resets it to Cash.

Stage 3's fourth option, Credit, is gone: the mockup specifies three.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `F2` | Open the item search, without reaching for Add Item. |
| `F5` | Print & Lock. |
| `F9` | Replicator Settings — **stub**, prints a status line; it belongs to the sync worker. |
| `Esc` | Clear the in-progress transaction, after confirming if anything has been billed. |

A footer bar above the status line advertises all four.

`Esc` is layered: it closes the print overlay, the invoice detail or the item picker
first, and only clears the counter when none of those are open — each of those handlers
calls `stopImmediatePropagation`, so one press never both closes an overlay and wipes the
transaction. On a locked (saved) card `Esc` does nothing: New Transaction is the way on.
On the login screen no shortcut fires at all, `Esc` included.

## Polish

Tabular figures across the item table and summary so columns do not drift; even padding on
table rows; a hover tint on billed rows; more room between the tax lines and the grand
total; consistent card padding. The dark palette is unchanged — it was already the
mockup's.

## Verified

Launched against a wiped `db.sqlite`:

1. The login screen is the first and only thing rendered — no shell, no tabs.
2. A wrong password gives "Incorrect username or password." and stays put.
3. The seeded `admin` account signs in with the password printed to the terminal; the
   title bar reads `REALINVOICE DESKTOP [Node: POS-01] Store Owner · owner` with a
   Sign Out button.
4. `F2` opened the item search.
5. Three invoices billed, one per payment type. In `db.sqlite`:

   | Invoice | payment_type | created_by_user_id | user |
   | --- | --- | --- | --- |
   | RI-2026-0001 | `upi` | 1 | admin / Store Owner |
   | RI-2026-0002 | `cash` | 1 | admin / Store Owner |
   | RI-2026-0003 | `card` | 1 | admin / Store Owner |

   The stored `password_hash` is a 60-character `$2b$12$…` bcrypt hash; the plaintext
   appears nowhere, and `sync_queue` holds zero `users` rows.
6. `Esc` on an empty screen said "Nothing to clear."; with an item billed it asked
   "Clear this transaction? 1 item(s) will be discarded." — Cancel kept the transaction,
   OK cleared it.
7. Sign Out returned to the login screen; `F2`, `F5`, `F9` and `Esc` all did nothing
   there and the shell was unreachable.
8. Restarting **without** wiping showed no first-run panel and no reseed, and the same
   account still signed in.

## Not in this stage

- User management UI (Settings > Users), password reset, or changing your own password.
- Role-based permissions. `owner` and `cashier` are recorded and displayed; nothing
  branches on them yet.
- Lockout or rate limiting on repeated failed sign-ins.
- Real UPI QR generation.
- F9 Replicator Settings, which needs the sync worker.
