# Office Console — Stage 15 (login rate limiting)

`verify_login` had no limit on failed attempts. A till sitting on a shop counter could be
guessed at indefinitely, as fast as bcrypt would answer.

## Where the limit lives

**In core, not in the command layer**, and `verify_login` is now private. That is the
whole design: a caller who could reach the raw credential check would silently skip the
limiter, which is precisely the bug being fixed. `Db::attempt_login` is the only way in,
so the Phoenix and Ratatui runtimes get the same protection without reimplementing it —
and a test asserts the desktop signs in through that path rather than keeping its own
check.

The order is: normalise the username → check the lockout → *only then* verify the
password. A locked-out username costs no bcrypt work at all.

## Decisions worth recording

**One bucket per username, however it is typed.** The limiter and the credential check
share `normalise_username`. If they disagreed, `priya`, `PRIYA` and ` Priya ` would be one
account to sign in as and three buckets to count against, and five attempts each would be
fifteen.

**Usernames that do not exist lock out on the same terms.** Otherwise the lockout becomes
an oracle: a real account would eventually start answering "locked", an imaginary one
never would, and the feature meant to protect accounts would enumerate them.

**A success does not clear earlier failures.** Letting one correct sign-in wipe the slate
would mean anybody who knew any one password on the machine could reset the limiter for
every other account at will. Failures age out on their own, and nothing resets the window
early — not a success, and not restarting the app, because the record is rows in SQLite.

**The deadline is set by the oldest failure in the window**, not a fixed period from the
last attempt. Measuring from "now" would let every fresh attempt push the deadline back,
and the lockout would never end.

## Two bugs the tests and the screen caught

**Refused attempts were extending the lockout.** The first cut recorded a locked-out
attempt as a failure, and the limiter counted it — so hammering a shut account kept it
shut forever, turning a rate limit into a denial of service against the real owner. The
test written for exactly that caught it. `login_attempts` now carries a fourth column,
`refused`: a fourth column rather than a third state in `success` because it answers a
different question — `success` says whether the credentials were right, `refused` says
whether they were even looked at. Refused attempts are logged and not counted.

**A lockout disabled the whole sign-in form.** Seen on screen, not in a test: with the
owner locked out, the cashier could not type their own name. The lockout is on a name, not
on the machine, so the username box stays usable and typing a different one lifts the
display. Core still decides; the screen only stops showing one account's lockout while
somebody types another's.

## What the screen says

Never a generic "invalid credentials" for a locked account — telling somebody their
password is wrong when it is right sends them hunting for a mistake they have not made.

| State | Message |
| --- | --- |
| 1st–3rd failure | "Incorrect username or password." |
| 4th | "… 2 attempts left before this account is locked for 15 minutes." |
| 5th | "… 1 attempt left …" |
| Locked | "Too many failed attempts. Try again in **12:29**." — counting down every second |

The warning starts only when it is nearly spent. A warning on every typo trains people to
ignore it, and the one that matters is the last. The countdown ticks in seconds because
one that only moves once a minute looks stuck, and re-enables the form by itself at zero.

## No secret is recorded

The table holds a username, a timestamp and two flags. No password, no hash, no fragment
of either — a failed-login log that recorded what was typed would be a second place
credentials leak from, including the near-misses, which are the most valuable kind. A test
asserts the serialised log contains neither the password that worked nor the one that did
not. Like `users` and `settings`, none of it is queued for sync.

## Verified

Driven through the real sign-in screen under Xvfb:

1. Five wrong passwords for `priya`; the fourth warned "1 attempt left".
2. The sixth attempt used the **correct** password and was refused: *"Too many failed
   attempts. Try again in 14:34."* — counting down to 14:15 six seconds later, with the
   password box and button disabled and typing into them ignored.
3. Restarted the app: still locked, at 12:29 — real time had kept running, and closing the
   app is not a way out.
4. Typed `meena` instead: the form freed itself and the cashier signed in normally while
   the owner stayed locked.
5. Aged the recorded attempts past the window: `priya` signed in with no further steps,
   and the rows are still on record — aged out, not deleted.

114 tests pass; clippy is clean.

## Not done

The window and the threshold are constants, not settings — a shop wanting three attempts
or thirty minutes needs a rebuild. Nothing prunes `login_attempts`, so like `sync_queue` it
grows forever. And there is no lockout on the first-run setup screen, because there is no
account to lock out yet.
