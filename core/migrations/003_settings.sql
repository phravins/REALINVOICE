-- 003_settings: local key/value preferences.
--
-- Machine-local UI state (which theme this till uses), not business data. Like `users`,
-- nothing here is queued for sync: one counter's display preference is not something the
-- back office should receive, let alone impose on another node.

CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
