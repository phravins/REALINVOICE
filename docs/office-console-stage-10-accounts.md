# Office Console — Stage 10 (real account creation)

Stage 5 got sign-in working by inventing an account: on a first run the app created
`admin`, generated a password, printed it to stdout, and — once the app was launched from
a desktop icon with no terminal behind it — showed it once on the login screen. That was
scaffolding to make the session gate real. This stage removes it, and lets the person
sitting in front of the machine choose their own credentials.

## What changed in principle

An installation with no users is now **unconfigured**, not broken and not pre-seeded.
Nothing about the first account is generated, printed, or hardcoded; the first password
only ever exists as the bcrypt hash the owner causes to be written. Demo customers and
items are still seeded, because those are sample data, not credentials.

Removed outright:

| Gone | Was |
| --- | --- |
| `seed::seed_owner_if_empty` | Created `admin` on a first run |
| `seed::DEFAULT_OWNER_USERNAME` | The string `"admin"` |
| `auth::generate_initial_password` | 16 random characters |
| `AppState::first_run_password` | Held that password in memory until sign-in |
| `AuthStatus::first_run_password` / `first_run_username` | Showed it on the login screen |

## The three screens

**Setup.** `auth_status` now reports `needs_setup` — true when `count_users() == 0`. The
shell shows *Create your account* in place of the gate: Display Name, Username, Password,
Confirm Password, in the same unboxed treatment as the login screen. `create_first_user`
hashes, inserts with role `owner`, and **begins the session in the same call**, so nobody
is asked to re-type credentials they chose ten seconds ago.

That command is unauthenticated by necessity — there is nobody to authenticate against
yet — so it is guarded by the only thing that makes it safe: it refuses outright once an
account exists, and its check and insert share the database lock, so a second caller
racing the first finds the table populated and is turned away.

**Login.** Unchanged, deliberately. It simply no longer has a first-run notice to show.

**Users.** A sidebar item below Settings, listing every account in the About page's
label/value rows — Display Name, Username, Role, Created — with an *Add User* form that
takes a role of Owner or Cashier. There is no edit and no delete: changing somebody's
password or removing their access is a thing to get right, not to bolt on.

## The owner gate

`list_users` and `create_user` go through `require_owner`, which is `require_session`
plus a role check. Hiding the Users item from cashiers is the courtesy; this is the
control. Both gates are exposed as `require_*_for_test` so the refusals are covered by
tests — a Tauri command cannot be called outside a running app, but the decision it makes
can.

Duplicate usernames are refused by a `UNIQUE` constraint in core. The command checks
first anyway, purely so the screen can say *"That username is already taken."* rather
than core's field-level wording. The constraint is still what guarantees it.

## Verified

Wiped `~/.config/in.osworks.realinvoice/db.sqlite` and drove the app under Xvfb:

1. First launch showed **Create your account**, not the login screen.
2. Creating *Priya Raman / priya* landed straight in the shell as the owner, with
   **Users** in the sidebar. SQLite: one row, role `owner`, `password_hash` starting
   `$2b$12$`, and **nothing** in `sync_queue` for `users` — only the 5 demo customers and
   7 demo items. Accounts are per-machine and are not synced.
3. Settings > Users listed the owner; *Add User* created *Meena R / meena* as a Cashier.
4. Mismatched passwords were refused client-side (*"The two passwords do not match."*)
   with nothing written; `MEENA` was refused against the existing `meena`
   (*"That username is already taken."*), so the check is case-insensitive.
5. Signed out, signed in as **meena**: the sidebar ends at Settings — no Users item.
6. Signed back in as **priya**: both accounts listed, oldest first.
7. Both themes, and the whole flow again at the 780×540 minimum window size, where the
   setup form still fits and the Add User grid reflows to two columns.

86 tests pass (`cargo test --workspace`), including the new coverage: a fresh database
has no accounts at all and no default `admin`; the account created at setup can sign in;
the user list carries no hash; and a cashier session is refused by `require_owner` while
still passing `require_session`.

## Not done

No password change, no password reset, no deactivating an account, and no audit of who
created whom. `docs/INSTALL.md` says plainly that a lost password cannot yet be reset.
