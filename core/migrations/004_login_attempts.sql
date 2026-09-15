-- 004_login_attempts: every sign-in attempt on this node, for rate limiting.
--
-- Attempts are recorded for any username that is typed, whether or not such an account
-- exists. That is deliberate: if only real usernames could ever lock out, the lockout
-- itself would tell an attacker which accounts are real.
--
-- Like `users` and `settings`, nothing here is queued for sync. Who mistyped a password
-- at this counter is local operational detail, not something the back office needs, and
-- shipping it would turn a failed-login log into a second place credentials-adjacent data
-- can leak from.
--
-- No password, hash or fragment of either is stored — only the username, the moment, and
-- whether it worked.

CREATE TABLE login_attempts (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Normalised the same way sign-in normalises it (trimmed, lowercased), so "PRIYA"
    -- and " priya " count against one bucket rather than three.
    username     TEXT    NOT NULL,
    attempted_at TEXT    NOT NULL DEFAULT (datetime('now')),
    success      INTEGER NOT NULL,
    -- Whether this attempt was turned away by the lockout without the password being
    -- checked at all.
    --
    -- A fourth column rather than a third state in `success`, because it answers a
    -- different question: `success` says whether the credentials were right, `refused`
    -- says whether they were even looked at. The limiter counts only genuine credential
    -- failures — if refusals counted too, somebody could hold a locked account shut
    -- indefinitely by attempting it while it was locked, turning the rate limit into a
    -- way of denying the real owner their till.
    refused      INTEGER NOT NULL DEFAULT 0
);

-- The lockout check runs on every attempt and reads one username over a short window.
CREATE INDEX idx_login_attempts_username_at ON login_attempts (username, attempted_at);
