//! The outbound push worker.
//!
//! Every write to `customers`, `items`, `invoices` or `invoice_lines` already drops a row
//! into `sync_queue` inside the same transaction, carrying the full record as JSON.
//! Nothing consumed that queue until now. This module drains it.
//!
//! # The one rule
//!
//! **Billing never waits for the network.** The worker owns no part of the write path: it
//! takes the shared connection only long enough to read a batch or mark one sent, and it
//! holds nothing across an HTTP call. A cloud that is down, slow, or wrong is a queue that
//! grows — never a till that stops. Everything below follows from that.
//!
//! # What gets sent
//!
//! One POST per batch, oldest row first. Order is load-bearing rather than tidy: a
//! customer is queued before the invoice that references it and an invoice before its
//! lines, so sending in insertion order means the far end never sees a row whose parent
//! has not arrived. A failed batch is retried as the same batch, so that order survives.
//!
//! The wire format is documented in `docs/sync-protocol.md` and pinned by tests. The
//! Back-Office Web does not exist yet, so nothing has been matched against real Ecto
//! schemas — the contract in that document is the thing stage 2 should be built against,
//! not the other way round.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::Result;
use crate::models::SyncQueueRow;

/// Rows per POST. A counter that has been offline all day should catch up in several
/// modest requests rather than one that times out halfway through.
pub const DEFAULT_BATCH_SIZE: usize = 50;

/// How often the worker looks for work when everything is healthy.
pub const DEFAULT_POLL_SECONDS: u64 = 10;

/// The ceiling on backoff. Long enough to stop hammering a dead endpoint, short enough
/// that a till which has been offline overnight recovers on its own within minutes of the
/// link coming back.
pub const MAX_BACKOFF_SECONDS: u64 = 300;

/// Placeholder credential.
///
/// Real per-node token issuance is a later hardening stage. Until then every node sends
/// the same string, which authenticates nothing — the endpoint it is sent to must be
/// treated as untrusted, and this must not be mistaken for a security boundary.
pub const PLACEHOLDER_TOKEN: &str = "realinvoice-dev-token";

/// Settings keys. Stored in core's `settings` table, so the endpoint survives a restart
/// and is editable from the Sync screen without a rebuild.
pub const ENDPOINT_KEY: &str = "sync.endpoint";
pub const TOKEN_KEY: &str = "sync.token";

/// How the worker is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConfig {
    /// Where batches are POSTed. Empty means "not configured yet", and the worker idles
    /// instead of failing in a loop against a URL nobody set.
    pub endpoint: String,
    /// Which till this is. Travels in the body and in a header.
    pub node_id: String,
    pub token: String,
    pub poll_interval: Duration,
    pub batch_size: usize,
    pub max_backoff: Duration,
}

impl SyncConfig {
    /// Reads what is stored, falling back to the defaults above.
    pub fn load(db: &Db, node_id: &str) -> Result<Self> {
        Ok(Self {
            endpoint: db.get_setting(ENDPOINT_KEY)?.unwrap_or_default(),
            node_id: node_id.to_string(),
            token: db.get_setting(TOKEN_KEY)?.unwrap_or_else(|| PLACEHOLDER_TOKEN.to_string()),
            poll_interval: Duration::from_secs(DEFAULT_POLL_SECONDS),
            batch_size: DEFAULT_BATCH_SIZE,
            max_backoff: Duration::from_secs(MAX_BACKOFF_SECONDS),
        })
    }
}

/// What the badge and the Sync screen read. Cheap to clone; never holds the database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncStatus {
    pub endpoint: String,
    pub node_id: String,
    /// Rows still waiting, as of the last poll.
    pub pending: i64,
    /// Local time of the last batch the far end accepted, `YYYY-MM-DD HH:MM:SS`.
    pub last_success: Option<String>,
    /// Rows accepted in that batch.
    pub last_batch: usize,
    /// Why the last attempt failed, in the words the screen should show.
    pub last_error: Option<String>,
    /// Failed attempts since the last success. Zero when healthy.
    pub consecutive_failures: u32,
    /// Seconds the worker is currently waiting before its next attempt.
    pub retry_in_seconds: u64,
    /// False until an endpoint is configured.
    pub configured: bool,
    /// How often the worker polls when healthy, so the UI can decide what "recent" means
    /// without hardcoding a number the worker owns.
    pub poll_seconds: u64,
}

impl SyncStatus {
    fn new(config: &SyncConfig) -> Self {
        Self {
            endpoint: config.endpoint.clone(),
            node_id: config.node_id.clone(),
            pending: 0,
            last_success: None,
            last_batch: 0,
            last_error: None,
            consecutive_failures: 0,
            retry_in_seconds: 0,
            configured: !config.endpoint.trim().is_empty(),
            poll_seconds: config.poll_interval.as_secs().max(1),
        }
    }
}

/// One queued row, as it goes over the wire.
///
/// `payload` is the record itself, re-parsed from the stored JSON rather than passed
/// through as a string, so the far end receives an object and not a quoted blob.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncEnvelopeRow {
    /// The queue row's id on this node. Stable, and what makes a retry idempotent: the
    /// far end can key on `(node_id, id)` to recognise a batch it has already taken.
    pub id: i64,
    pub table_name: String,
    pub row_id: i64,
    pub op: String,
    pub recorded_at: String,
    pub payload: serde_json::Value,
}

impl SyncEnvelopeRow {
    /// Fails only if a queued payload is not valid JSON, which would mean something wrote
    /// the queue without going through `enqueue`.
    fn from_row(row: &SyncQueueRow) -> std::result::Result<Self, String> {
        let payload: serde_json::Value = serde_json::from_str(&row.payload_json)
            .map_err(|e| format!("queue row {} holds invalid JSON: {e}", row.id))?;
        Ok(Self {
            id: row.id,
            table_name: row.table_name.clone(),
            row_id: row.row_id,
            op: row.op.clone(),
            recorded_at: row.created_at.clone(),
            payload,
        })
    }
}

/// The POST body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncBatch {
    pub node_id: String,
    pub sent_at: String,
    pub rows: Vec<SyncEnvelopeRow>,
}

impl SyncBatch {
    /// Builds the body for a batch of queued rows.
    pub fn build(node_id: &str, rows: &[SyncQueueRow]) -> std::result::Result<Self, String> {
        Ok(Self {
            node_id: node_id.to_string(),
            sent_at: chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
            rows: rows
                .iter()
                .map(SyncEnvelopeRow::from_row)
                .collect::<std::result::Result<_, _>>()?,
        })
    }

    /// The queue ids this batch covers, to mark once it is accepted.
    pub fn ids(&self) -> Vec<i64> {
        self.rows.iter().map(|r| r.id).collect()
    }
}

/// Doubling backoff, capped.
///
/// Starts at the poll interval rather than at zero: the first retry after a failure is
/// the next ordinary poll, and only a run of failures backs further off.
pub fn backoff_for(failures: u32, poll: Duration, max: Duration) -> Duration {
    if failures == 0 {
        return poll;
    }
    let factor = 2u64.saturating_pow(failures.saturating_sub(1).min(16));
    let secs = poll.as_secs().saturating_mul(factor);
    Duration::from_secs(secs.min(max.as_secs()))
}

/// Shared, readable state plus the nudge the "Sync Now" button sends.
#[derive(Clone)]
pub struct SyncHandle {
    status: Arc<Mutex<SyncStatus>>,
    #[cfg(feature = "sync")]
    wake: Arc<tokio::sync::Notify>,
}

impl SyncHandle {
    /// A snapshot for the UI.
    pub fn status(&self) -> SyncStatus {
        self.status.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Asks the worker to poll now instead of waiting out its timer. Returns immediately;
    /// the button must not block the screen on a network round trip.
    #[cfg(feature = "sync")]
    pub fn sync_now(&self) {
        self.wake.notify_one();
    }

    /// Replaces the endpoint the status reports, after it is changed on the Sync screen.
    /// The worker re-reads the stored value at the top of each attempt.
    pub fn set_endpoint(&self, endpoint: &str) {
        let mut status = self.status.lock().unwrap_or_else(|p| p.into_inner());
        status.endpoint = endpoint.to_string();
        status.configured = !endpoint.trim().is_empty();
    }
}

#[cfg(feature = "sync")]
mod worker {
    use super::*;

    /// Builds the worker and the handle the UI reads, without scheduling anything.
    ///
    /// The caller spawns the future on whatever runtime it already has — the desktop uses
    /// Tauri's, which is where its own commands run. Core does not reach for a runtime of
    /// its own: a library that calls `tokio::spawn` only works inside a context it cannot
    /// see, and panics everywhere else.
    ///
    /// `db` is shared with the rest of the app. The worker locks it to read a batch and
    /// to mark one sent, and never across an `.await` — a slow endpoint must not be able
    /// to hold the lock the billing screen needs to save an invoice.
    pub fn start(
        db: Arc<Mutex<Db>>,
        config: SyncConfig,
    ) -> (SyncHandle, impl std::future::Future<Output = ()> + Send + 'static) {
        let handle = SyncHandle {
            status: Arc::new(Mutex::new(SyncStatus::new(&config))),
            wake: Arc::new(tokio::sync::Notify::new()),
        };

        let worker = handle.clone();
        (handle, async move { run(db, config, worker).await })
    }

    async fn run(db: Arc<Mutex<Db>>, config: SyncConfig, handle: SyncHandle) {
        // One client for the process: it pools connections, and building one per attempt
        // would re-do TLS setup every ten seconds.
        let client = match reqwest::Client::builder().timeout(Duration::from_secs(20)).build() {
            Ok(client) => client,
            Err(err) => {
                set(&handle, |s| {
                    s.last_error = Some(format!("could not start the sync client: {err}"));
                });
                return;
            }
        };

        let mut failures: u32 = 0;

        loop {
            let wait = backoff_for(failures, config.poll_interval, config.max_backoff);
            set(&handle, |s| s.retry_in_seconds = wait.as_secs());

            // Either the timer or the Sync Now button, whichever comes first.
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = handle.wake.notified() => {}
            }

            match attempt(&db, &config, &client, &handle).await {
                Outcome::Idle | Outcome::Sent => failures = 0,
                Outcome::Failed => failures = failures.saturating_add(1),
            }
        }
    }

    enum Outcome {
        /// No endpoint configured, so there was nothing to attempt.
        Idle,
        Sent,
        Failed,
    }

    async fn attempt(
        db: &Arc<Mutex<Db>>,
        config: &SyncConfig,
        client: &reqwest::Client,
        handle: &SyncHandle,
    ) -> Outcome {
        // Read the endpoint every time rather than caching it: the Sync screen can change
        // it while the worker is running, and the next attempt should use the new one.
        let (endpoint, token, pending, batch) = {
            let db = db.lock().unwrap_or_else(|p| p.into_inner());
            let endpoint = db
                .get_setting(ENDPOINT_KEY)
                .ok()
                .flatten()
                .unwrap_or_else(|| config.endpoint.clone());
            let token =
                db.get_setting(TOKEN_KEY).ok().flatten().unwrap_or_else(|| config.token.clone());
            let pending = db.pending_sync_count().unwrap_or(0);
            let batch = db.next_sync_batch(config.batch_size).unwrap_or_default();
            (endpoint, token, pending, batch)
        };

        set(handle, |s| {
            s.pending = pending;
            s.endpoint = endpoint.clone();
            s.configured = !endpoint.trim().is_empty();
        });

        if endpoint.trim().is_empty() {
            // Not an error: a till that has not been pointed at a back office yet is
            // waiting to be configured, not failing. Saying "failed" here would turn the
            // badge red on every fresh install.
            set(handle, |s| {
                s.last_error = None;
                s.consecutive_failures = 0;
            });
            return Outcome::Idle;
        }

        // An empty queue still sends — a batch with no rows is a heartbeat.
        //
        // The alternative, skipping the request when there is nothing to push, was tried
        // and is wrong: an idle till stops contacting the back office, its last success
        // ages past the badge's window, and a shop that is perfectly healthy and fully
        // up to date reads "Offline". Claiming a success that never happened would be
        // worse. So the worker actually checks, which also gives the far end a liveness
        // signal for a till that simply has not billed anything yet today.

        let body = match SyncBatch::build(&config.node_id, &batch) {
            Ok(body) => body,
            Err(err) => {
                // A malformed queue row would fail forever; skipping it silently would
                // lose a record. Report it and stop, so somebody looks.
                set(handle, |s| {
                    s.last_error = Some(err);
                    s.consecutive_failures = s.consecutive_failures.saturating_add(1);
                });
                return Outcome::Failed;
            }
        };
        let ids = body.ids();

        let sent = client
            .post(&endpoint)
            .header("authorization", format!("Bearer {token}"))
            .header("x-realinvoice-node", &config.node_id)
            .json(&body)
            .send()
            .await;

        match sent {
            Ok(response) if response.status().is_success() => {
                let marked = {
                    let mut db = db.lock().unwrap_or_else(|p| p.into_inner());
                    // Nothing to mark on a heartbeat, and mark_synced treats an empty
                    // slice as a no-op rather than a special case.
                    let marked = db.mark_synced(&ids).unwrap_or(0);
                    let pending = db.pending_sync_count().unwrap_or(0);
                    set(handle, |s| s.pending = pending);
                    marked
                };
                set(handle, |s| {
                    s.last_success = Some(now());
                    s.last_batch = marked;
                    s.last_error = None;
                    s.consecutive_failures = 0;
                });
                Outcome::Sent
            }
            Ok(response) => {
                // Any non-2xx leaves the batch queued. Nothing is marked, so the same
                // rows go again next time — the far end rejecting a batch must never cost
                // a record.
                let status = response.status();
                set(handle, |s| {
                    s.last_error = Some(format!("the back office answered {status}"));
                    s.consecutive_failures = s.consecutive_failures.saturating_add(1);
                });
                Outcome::Failed
            }
            Err(err) => {
                set(handle, |s| {
                    s.last_error = Some(describe(&err));
                    s.consecutive_failures = s.consecutive_failures.saturating_add(1);
                });
                Outcome::Failed
            }
        }
    }

    /// reqwest's own Display is written for developers. The cashier reads this.
    fn describe(err: &reqwest::Error) -> String {
        if err.is_timeout() {
            "the back office did not answer in time".to_string()
        } else if err.is_connect() {
            "could not reach the back office".to_string()
        } else {
            format!("could not send: {err}")
        }
    }

    fn set(handle: &SyncHandle, edit: impl FnOnce(&mut SyncStatus)) {
        let mut status = handle.status.lock().unwrap_or_else(|p| p.into_inner());
        edit(&mut status);
    }

    fn now() -> String {
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
    }
}

#[cfg(feature = "sync")]
pub use worker::start;

/// A handle for a build with the worker compiled out, so the UI has something to read.
#[cfg(not(feature = "sync"))]
pub fn idle_handle(config: &SyncConfig) -> SyncHandle {
    SyncHandle { status: Arc::new(Mutex::new(SyncStatus::new(config))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_from_the_poll_interval_and_stops_at_the_cap() {
        let poll = Duration::from_secs(10);
        let max = Duration::from_secs(300);

        // Healthy: the next attempt is the next ordinary poll.
        assert_eq!(backoff_for(0, poll, max), Duration::from_secs(10));
        // And the first retry is too — one failure should not push a till five minutes out.
        assert_eq!(backoff_for(1, poll, max), Duration::from_secs(10));
        assert_eq!(backoff_for(2, poll, max), Duration::from_secs(20));
        assert_eq!(backoff_for(3, poll, max), Duration::from_secs(40));
        assert_eq!(backoff_for(4, poll, max), Duration::from_secs(80));
        assert_eq!(backoff_for(5, poll, max), Duration::from_secs(160));
        // Capped, and stays capped however long the outage lasts.
        assert_eq!(backoff_for(6, poll, max), max);
        assert_eq!(backoff_for(40, poll, max), max);
        // No overflow panic on an absurd failure count.
        assert_eq!(backoff_for(u32::MAX, poll, max), max);
    }
}
