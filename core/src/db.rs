//! `Db` — the only way into the SQLite file.
//!
//! Two rules hold across every function here:
//!
//! 1. Every write to `customers`, `items`, `invoices` or `invoice_lines` also inserts a
//!    row into `sync_queue`, in the *same* transaction. Either both land or neither does,
//!    so the queue can never drift from the data it describes.
//! 2. Nothing here touches the network. The queue is written and left alone; draining it
//!    is a later stage.

use chrono::{Local, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde::Serialize;

use crate::auth;
use crate::error::{CoreError, Result};
use crate::gst::{self, TaxableLine, DEFAULT_HOME_STATE};
use crate::models::*;
use crate::numbering::{financial_year, next_credit_note_no, next_invoice_no};
use crate::schema::run_migrations;

/// A connection to one RealInvoice SQLite file, with migrations already applied.
pub struct Db {
    conn: Connection,
    /// Seller's state, used for the CGST/SGST vs IGST decision.
    home_state: String,
}

impl Db {
    /// Opens (creating if needed) the database at `path` and runs migrations.
    pub fn open<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    CoreError::Invalid(format!("cannot create {}: {e}", parent.display()))
                })?;
            }
        }
        Self::from_connection(Connection::open(path)?)
    }

    /// An ephemeral database. Used by tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(mut conn: Connection) -> Result<Self> {
        run_migrations(&mut conn)?;
        Ok(Self { conn, home_state: DEFAULT_HOME_STATE.to_string() })
    }

    /// Overrides the seller's home state (default `TN`).
    pub fn with_home_state(mut self, state: impl Into<String>) -> Self {
        self.home_state = state.into();
        self
    }

    pub fn home_state(&self) -> &str {
        &self.home_state
    }

    // ----------------------------------------------------------------- settings

    /// Reads a local preference. `None` when it has never been set.
    ///
    /// Machine-local UI state only — no business data lives here, and nothing in this
    /// table is queued for sync.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| row.get(0))
            .optional()?)
    }

    /// Writes a local preference, replacing any previous value.
    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<()> {
        if key.trim().is_empty() {
            return Err(CoreError::Invalid("setting key is required".into()));
        }
        self.conn.execute(
            "INSERT INTO settings (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT (key) DO UPDATE SET value = excluded.value,
                                             updated_at = excluded.updated_at",
            params![key.trim(), value],
        )?;
        Ok(())
    }

    // -------------------------------------------------------------------- users

    /// Registers a user. The password is hashed here and the plaintext is dropped with
    /// this call — nothing stores or returns it.
    ///
    /// No `sync_queue` row is written. Every other write queues, but replicating password
    /// hashes off this machine is a decision for whoever builds the sync worker; starting
    /// to do it by default would make that choice silently.
    pub fn create_user(&mut self, new: &NewUser, password: &str) -> Result<User> {
        let username = new.username.trim().to_lowercase();
        if username.is_empty() {
            return Err(CoreError::Invalid("username is required".into()));
        }
        if new.display_name.trim().is_empty() {
            return Err(CoreError::Invalid("display_name is required".into()));
        }
        if self.find_user(&username)?.is_some() {
            return Err(CoreError::Invalid(format!("user {username} already exists")));
        }

        let password_hash = auth::hash_password(password)?;
        self.conn.execute(
            "INSERT INTO users (username, password_hash, display_name, role)
             VALUES (?1, ?2, ?3, ?4)",
            params![username, password_hash, new.display_name.trim(), new.role.as_str()],
        )?;

        self.get_user(self.conn.last_insert_rowid())?
            .ok_or_else(|| CoreError::NotFound("user just created".into()))
    }

    /// Checks a sign-in. `None` for both an unknown username and a wrong password — the
    /// caller cannot tell which, so the screen cannot leak which usernames exist.
    ///
    /// The hash is always verified, even when no such user exists, so the call takes the
    /// same time either way and cannot be used to enumerate accounts.
    /// The raw credential check, with **no** rate limiting.
    ///
    /// Private on purpose. [`Db::attempt_login`] is the way in; a caller that could reach
    /// this directly would skip the limiter, which is the whole thing being defended
    /// against. Kept separate rather than inlined so the timing-equalising dummy verify
    /// below stays one idea in one place.
    fn verify_login(&self, username: &str, password: &str) -> Result<Option<User>> {
        let username = normalise_username(username);
        let found: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT id, password_hash FROM users WHERE username = ?1",
                [&username],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        match found {
            Some((id, password_hash)) => {
                if auth::verify_password(password, &password_hash) {
                    self.get_user(id)
                } else {
                    Ok(None)
                }
            }
            None => {
                // A dummy verify against a real hash, so a missing user costs the same
                // time as a wrong password.
                auth::verify_password(password, DUMMY_HASH);
                Ok(None)
            }
        }
    }

    // ------------------------------------------------------------ sign-in limits

    /// Signs in, subject to the rate limit. **The only way to sign in.**
    ///
    /// [`Db::verify_login`] is deliberately not public: a caller that reached the raw
    /// credential check directly would silently skip the limit, and that is exactly the
    /// bug this exists to prevent. Every runtime — desktop, web, TUI — goes through here
    /// and gets the same behaviour without reimplementing it.
    ///
    /// The order matters. The lockout is checked *before* the password, so a locked-out
    /// username costs no bcrypt verification at all. That also means a locked-out attempt
    /// returns noticeably faster than a normal one, which reveals that *this username* is
    /// locked out — but not whether it exists, because a username nobody has registered
    /// locks out on exactly the same terms.
    pub fn attempt_login(&mut self, username: &str, password: &str) -> Result<LoginOutcome> {
        let key = normalise_username(username);

        let lockout = self.lockout_for(&key)?;
        if let Some(lockout) = lockout {
            // Recorded as *refused*, which is logged but not counted. The row is the
            // audit trail of somebody hammering a locked account; counting it would let an
            // attacker keep a username locked out forever by attempting it while shut out,
            // turning the rate limit into a denial of service against the real owner.
            self.record_login_attempt(&key, false, true)?;
            return Ok(LoginOutcome::LockedOut(lockout));
        }

        let found = self.verify_login(&key, password)?;
        self.record_login_attempt(&key, found.is_some(), false)?;

        Ok(match found {
            Some(user) => LoginOutcome::Ok(user),
            None => LoginOutcome::Invalid,
        })
    }

    /// Whether this username is currently shut out, and for how much longer.
    ///
    /// Counts failures inside the window only. A success does **not** clear them: letting
    /// one correct sign-in wipe the slate would mean an attacker who knows any one
    /// password on the machine could reset the limiter for every other account at will.
    /// Failures age out on their own instead.
    pub fn lockout_for(&self, username: &str) -> Result<Option<Lockout>> {
        let key = normalise_username(username);
        let window = format!("-{LOGIN_WINDOW_MINUTES} minutes");

        let (failures, oldest): (i64, Option<String>) = self.conn.query_row(
            "SELECT COUNT(*), MIN(attempted_at)
               FROM login_attempts
              WHERE username = ?1
                AND success = 0
                AND refused = 0
                AND attempted_at >= datetime('now', ?2)",
            params![key, window],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        if failures < MAX_FAILED_LOGINS {
            return Ok(None);
        }

        // The lock lifts when the oldest failure in the window ages out, not a fixed
        // period from now — otherwise every fresh attempt would push the deadline back and
        // the lockout would never end.
        let retry_after_seconds = match oldest {
            Some(oldest) => self
                .conn
                .query_row(
                    "SELECT CAST(strftime('%s', ?1, ?2) AS INTEGER)
                            - CAST(strftime('%s', 'now') AS INTEGER)",
                    params![oldest, format!("+{LOGIN_WINDOW_MINUTES} minutes")],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0)
                .max(0),
            None => 0,
        };

        Ok(Some(Lockout { failures, remaining: 0, retry_after_seconds }))
    }

    /// Records an attempt. Every attempt is recorded, for usernames that exist and
    /// usernames that do not.
    ///
    /// `refused` marks an attempt the lockout turned away before the password was
    /// checked. Those are kept as a trail of somebody hammering a shut account, and
    /// deliberately do not count towards the limit.
    pub fn record_login_attempt(
        &mut self,
        username: &str,
        success: bool,
        refused: bool,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO login_attempts (username, success, refused) VALUES (?1, ?2, ?3)",
            params![
                normalise_username(username),
                if success { 1 } else { 0 },
                if refused { 1 } else { 0 }
            ],
        )?;
        Ok(())
    }

    /// Failed attempts for this username inside the window. Used by the screen to warn on
    /// the way to a lockout, and by the tests.
    pub fn recent_failed_logins(&self, username: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*)
               FROM login_attempts
              WHERE username = ?1
                AND success = 0
                AND refused = 0
                AND attempted_at >= datetime('now', ?2)",
            params![normalise_username(username), format!("-{LOGIN_WINDOW_MINUTES} minutes")],
            |row| row.get(0),
        )?)
    }

    /// Ages this username's recorded attempts by `minutes`, as the clock would.
    ///
    /// Test-only, and named so nobody reaches for it by accident. It exists because the
    /// alternative — proving the window expires by waiting fifteen minutes — is a test
    /// nobody will run, and a rate limit whose expiry is never tested is a rate limit that
    /// might never expire.
    #[doc(hidden)]
    pub fn backdate_login_attempts_for_test(&mut self, username: &str, minutes: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE login_attempts
                SET attempted_at = datetime(attempted_at, ?2)
              WHERE username = ?1",
            params![normalise_username(username), format!("-{minutes} minutes")],
        )?;
        Ok(())
    }

    /// Every recorded attempt, oldest first. For tests and any future audit screen.
    pub fn login_attempts(&self, username: &str) -> Result<Vec<LoginAttempt>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, username, attempted_at, success, refused
               FROM login_attempts
              WHERE username = ?1
              ORDER BY id",
        )?;
        let rows = stmt.query_map([normalise_username(username)], |row| {
            Ok(LoginAttempt {
                id: row.get(0)?,
                username: row.get(1)?,
                attempted_at: row.get(2)?,
                success: row.get::<_, i64>(3)? != 0,
                refused: row.get::<_, i64>(4)? != 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_user(&self, id: i64) -> Result<Option<User>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, username, display_name, role, created_at FROM users WHERE id = ?1",
                [id],
                user_from_row,
            )
            .optional()?)
    }

    /// Looks a user up by username, case-insensitively. No password check.
    pub fn find_user(&self, username: &str) -> Result<Option<User>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, username, display_name, role, created_at
                   FROM users WHERE username = ?1",
                [username.trim().to_lowercase()],
                user_from_row,
            )
            .optional()?)
    }

    /// Every account, oldest first. Owner-only in the UI; core does not gate it.
    pub fn list_users(&self) -> Result<Vec<User>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, username, display_name, role, created_at FROM users ORDER BY id",
        )?;
        let rows = stmt.query_map([], user_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// How many accounts exist. Zero means the app has never been set up.
    pub fn count_users(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?)
    }

    /// Replaces a user's password. No reset flow exists yet; this is the primitive one
    /// will be built on.
    pub fn set_password(&mut self, user_id: i64, password: &str) -> Result<()> {
        let password_hash = auth::hash_password(password)?;
        let changed = self.conn.execute(
            "UPDATE users SET password_hash = ?1 WHERE id = ?2",
            params![password_hash, user_id],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!("user {user_id}")));
        }
        Ok(())
    }

    // ---------------------------------------------------------------- customers

    /// Looks a customer up by exact mobile number. The counter's fastest path: the
    /// operator types a number and either gets a name back or creates one.
    pub fn search_customer(&self, mobile: &str) -> Result<Option<Customer>> {
        let mobile = mobile.trim();
        if mobile.is_empty() {
            return Ok(None);
        }
        let found = self
            .conn
            .query_row(
                "SELECT id, name, gstin, place_of_supply, mobile
                   FROM customers
                  WHERE mobile = ?1",
                [mobile],
                customer_from_row,
            )
            .optional()?;
        Ok(found)
    }

    /// Registers a brand-new customer. Fails if that mobile number is already on file —
    /// the counter has resolved it and found nothing, so a collision here means someone
    /// else registered it in between and the operator should see the existing record
    /// rather than silently overwrite it.
    pub fn create_customer(&mut self, new: &NewCustomer) -> Result<Customer> {
        let mobile = new.mobile.trim().to_string();
        if mobile.is_empty() {
            return Err(CoreError::Invalid("customer mobile is required".into()));
        }
        if new.name.trim().is_empty() {
            return Err(CoreError::Invalid("customer name is required".into()));
        }
        if new.place_of_supply.trim().is_empty() {
            return Err(CoreError::Invalid("place_of_supply is required".into()));
        }
        if self.search_customer(&mobile)?.is_some() {
            return Err(CoreError::Invalid(format!("mobile {mobile} is already registered")));
        }

        let gstin =
            new.gstin.as_ref().map(|g| g.trim()).filter(|g| !g.is_empty()).map(String::from);
        let place_of_supply = new.place_of_supply.trim().to_uppercase();

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO customers (name, gstin, place_of_supply, mobile)
             VALUES (?1, ?2, ?3, ?4)",
            params![new.name.trim(), gstin, place_of_supply, mobile],
        )?;
        let customer = Customer {
            id: tx.last_insert_rowid(),
            name: new.name.trim().to_string(),
            gstin,
            place_of_supply,
            mobile,
        };
        enqueue(&tx, "customers", customer.id, SyncOp::Insert, &customer)?;
        tx.commit()?;
        Ok(customer)
    }

    /// Inserts a customer, or updates the existing one with that mobile number.
    pub fn upsert_customer(&mut self, new: &NewCustomer) -> Result<Customer> {
        let mobile = new.mobile.trim().to_string();
        if mobile.is_empty() {
            return Err(CoreError::Invalid("customer mobile is required".into()));
        }

        let existing = self.search_customer(&mobile)?;
        let op = if existing.is_some() { SyncOp::Update } else { SyncOp::Insert };
        let tx = self.conn.transaction()?;

        let customer = match existing {
            Some(current) => {
                tx.execute(
                    "UPDATE customers
                        SET name = ?1, gstin = ?2, place_of_supply = ?3
                      WHERE id = ?4",
                    params![new.name, new.gstin, new.place_of_supply, current.id],
                )?;
                Customer {
                    id: current.id,
                    name: new.name.clone(),
                    gstin: new.gstin.clone(),
                    place_of_supply: new.place_of_supply.clone(),
                    mobile,
                }
            }
            None => {
                tx.execute(
                    "INSERT INTO customers (name, gstin, place_of_supply, mobile)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![new.name, new.gstin, new.place_of_supply, mobile],
                )?;
                Customer {
                    id: tx.last_insert_rowid(),
                    name: new.name.clone(),
                    gstin: new.gstin.clone(),
                    place_of_supply: new.place_of_supply.clone(),
                    mobile,
                }
            }
        };

        enqueue(&tx, "customers", customer.id, op, &customer)?;
        tx.commit()?;
        Ok(customer)
    }

    /// The customer behind an id, for callers that already resolved one.
    pub fn get_customer(&self, id: i64) -> Result<Option<Customer>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, gstin, place_of_supply, mobile FROM customers WHERE id = ?1",
                [id],
                customer_from_row,
            )
            .optional()?)
    }

    // -------------------------------------------------------------------- items

    /// Substring search over item code and description, code first. An empty query
    /// lists the catalogue so the UI can show something before the operator types.
    pub fn search_item(&self, query: &str) -> Result<Vec<Item>> {
        let pattern = format!("%{}%", query.trim());
        let mut stmt = self.conn.prepare(
            "SELECT id, item_code, description, rate, tax_rate, uom
               FROM items
              WHERE item_code LIKE ?1 COLLATE NOCASE
                 OR description LIKE ?1 COLLATE NOCASE
              ORDER BY item_code
              LIMIT 50",
        )?;
        let rows = stmt.query_map([pattern], item_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Inserts an item, or updates the existing one with that item code.
    pub fn upsert_item(&mut self, new: &NewItem) -> Result<Item> {
        let code = new.item_code.trim().to_string();
        if code.is_empty() {
            return Err(CoreError::Invalid("item_code is required".into()));
        }

        let existing: Option<i64> = self
            .conn
            .query_row("SELECT id FROM items WHERE item_code = ?1", [&code], |r| r.get(0))
            .optional()?;
        let tx = self.conn.transaction()?;

        let (id, op) = match existing {
            Some(id) => {
                tx.execute(
                    "UPDATE items SET description = ?1, rate = ?2, tax_rate = ?3, uom = ?4
                      WHERE id = ?5",
                    params![new.description, new.rate, new.tax_rate, new.uom, id],
                )?;
                (id, SyncOp::Update)
            }
            None => {
                tx.execute(
                    "INSERT INTO items (item_code, description, rate, tax_rate, uom)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![code, new.description, new.rate, new.tax_rate, new.uom],
                )?;
                (tx.last_insert_rowid(), SyncOp::Insert)
            }
        };

        let item = Item {
            id,
            item_code: code,
            description: new.description.clone(),
            rate: new.rate,
            tax_rate: new.tax_rate,
            uom: new.uom.clone(),
        };
        enqueue(&tx, "items", item.id, op, &item)?;
        tx.commit()?;
        Ok(item)
    }

    /// The whole catalogue, or the part of it matching `text`.
    ///
    /// Separate from [`Db::search_item`], which feeds the billing screen's picker and is
    /// deliberately capped at 50: this one is the Inventory pane's list, where a shop with
    /// 900 items expects to see 900 items.
    pub fn list_items(&self, filter: &ItemFilter) -> Result<Vec<Item>> {
        let text = filter.text.as_deref().unwrap_or("").trim().to_string();
        let pattern = format!("%{text}%");
        let limit = filter.limit.unwrap_or(1000) as i64;

        let mut stmt = self.conn.prepare(
            "SELECT id, item_code, description, rate, tax_rate, uom
               FROM items
              WHERE ?1 = ''
                 OR item_code LIKE ?2 COLLATE NOCASE
                 OR description LIKE ?2 COLLATE NOCASE
              ORDER BY item_code
              LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![text, pattern, limit], item_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// How many items the catalogue holds, ignoring any filter.
    pub fn count_items(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))?)
    }

    pub fn get_item(&self, id: i64) -> Result<Option<Item>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, item_code, description, rate, tax_rate, uom FROM items WHERE id = ?1",
                [id],
                item_from_row,
            )
            .optional()?)
    }

    // ----------------------------------------------------------------- invoices

    /// Creates an invoice and its lines in one transaction: allocates the next number in
    /// the financial year, prices each line off the item master unless overridden, splits
    /// GST against the customer's place of supply, and queues a `sync_queue` row for the
    /// invoice and for every line.
    pub fn create_invoice(&mut self, new: &NewInvoice) -> Result<Invoice> {
        let customer = self
            .get_customer(new.customer_id)?
            .ok_or_else(|| CoreError::NotFound(format!("customer {}", new.customer_id)))?;

        let date = match &new.date {
            Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .map_err(|_| CoreError::Invalid(format!("date must be YYYY-MM-DD, got {d}")))?,
            None => Local::now().date_naive(),
        };

        // Price the lines before opening the transaction.
        let mut priced = Vec::with_capacity(new.lines.len());
        for line in &new.lines {
            priced.push(self.price_line(line)?);
        }

        let taxables: Vec<TaxableLine> =
            priced.iter().map(|(_, t): &(i64, TaxableLine)| *t).collect();
        let totals = gst::compute_totals(&taxables, &self.home_state, &customer.place_of_supply);

        // IMMEDIATE takes the write lock up front, so the number is read and used under
        // the same lock that inserts it. Allocating before the transaction would let two
        // consoles billing at the same moment read the same highest number and collide.
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let fy = financial_year(date);
        let invoice_no = next_invoice_no(fy, highest_invoice_no(&tx, fy)?.as_deref())?;

        // Stamped here rather than left to the column default, which is UTC: `date` is
        // the counter's local day, and a time from a different clock beside it misleads.
        let created_at = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

        tx.execute(
            "INSERT INTO invoices
                 (invoice_no, date, customer_id, subtotal, cgst, sgst, igst,
                  grand_total, payment_type, sync_status, created_at, created_by_user_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', ?10, ?11)",
            params![
                invoice_no,
                date.to_string(),
                customer.id,
                totals.subtotal,
                totals.cgst,
                totals.sgst,
                totals.igst,
                totals.grand_total,
                new.payment_type,
                created_at,
                new.created_by_user_id,
            ],
        )?;
        let invoice_id = tx.last_insert_rowid();

        let invoice = Invoice {
            id: invoice_id,
            invoice_no,
            date: date.to_string(),
            customer_id: customer.id,
            subtotal: totals.subtotal,
            cgst: totals.cgst,
            sgst: totals.sgst,
            igst: totals.igst,
            grand_total: totals.grand_total,
            payment_type: new.payment_type.clone(),
            sync_status: "pending".to_string(),
            created_at,
            created_by_user_id: new.created_by_user_id,
        };

        // Queued before its lines, not after. The sync worker sends in queue order, so a
        // line that travels ahead of the invoice it belongs to would arrive at the back
        // office referencing an invoice that is not there yet. Both still land in this one
        // transaction, so nothing about the local write changes.
        enqueue(&tx, "invoices", invoice.id, SyncOp::Insert, &invoice)?;

        for (item_id, taxable) in &priced {
            insert_line(&tx, invoice_id, *item_id, taxable)?;
        }

        tx.commit()?;
        Ok(invoice)
    }

    /// Appends a line to an existing invoice and re-totals it. Both the new line and the
    /// changed invoice are queued.
    pub fn add_line_item(&mut self, invoice_id: i64, line: &NewInvoiceLine) -> Result<InvoiceLine> {
        let invoice = self
            .get_invoice(invoice_id)?
            .ok_or_else(|| CoreError::NotFound(format!("invoice {invoice_id}")))?;
        let customer = self
            .get_customer(invoice.customer_id)?
            .ok_or_else(|| CoreError::NotFound(format!("customer {}", invoice.customer_id)))?;

        let (item_id, taxable) = self.price_line(line)?;
        let mut taxables: Vec<TaxableLine> = self
            .invoice_lines(invoice_id)?
            .iter()
            .map(|l| TaxableLine { qty: l.qty, rate: l.rate, tax_rate: l.tax_rate })
            .collect();
        taxables.push(taxable);
        let totals = gst::compute_totals(&taxables, &self.home_state, &customer.place_of_supply);

        let tx = self.conn.transaction()?;
        let line_id = insert_line(&tx, invoice_id, item_id, &taxable)?;
        tx.execute(
            "UPDATE invoices
                SET subtotal = ?1, cgst = ?2, sgst = ?3, igst = ?4,
                    grand_total = ?5, sync_status = 'pending'
              WHERE id = ?6",
            params![
                totals.subtotal,
                totals.cgst,
                totals.sgst,
                totals.igst,
                totals.grand_total,
                invoice_id
            ],
        )?;

        let updated = Invoice {
            subtotal: totals.subtotal,
            cgst: totals.cgst,
            sgst: totals.sgst,
            igst: totals.igst,
            grand_total: totals.grand_total,
            sync_status: "pending".to_string(),
            ..invoice
        };
        enqueue(&tx, "invoices", invoice_id, SyncOp::Update, &updated)?;
        tx.commit()?;

        Ok(InvoiceLine {
            id: line_id,
            invoice_id,
            item_id,
            qty: taxable.qty,
            rate: taxable.rate,
            tax_rate: taxable.tax_rate,
            line_total: taxable.line_total(),
        })
    }

    /// Every invoice raised today, newest first. The billing counter's day view.
    pub fn list_todays_invoices(&self) -> Result<Vec<Invoice>> {
        self.list_invoices_for_date(Local::now().date_naive())
    }

    /// Every invoice on a given date, newest first.
    pub fn list_invoices_for_date(&self, date: NaiveDate) -> Result<Vec<Invoice>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, invoice_no, date, customer_id, subtotal, cgst, sgst, igst,
                    grand_total, payment_type, sync_status, created_at, created_by_user_id
               FROM invoices
              WHERE date = ?1
              ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([date.to_string()], invoice_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The history query. Filters on date range and a substring of the customer name or
    /// invoice number; newest first. An empty filter lists everything.
    pub fn list_invoices(&self, filter: &InvoiceFilter) -> Result<Vec<InvoiceSummary>> {
        // `text` is matched with LIKE, so the wildcards have to be built here. A NULL
        // pattern short-circuits the whole clause rather than matching nothing.
        let pattern = filter
            .text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| format!("%{t}%"));
        let limit = filter.limit.unwrap_or(500);

        let mut stmt = self.conn.prepare(
            "SELECT i.id, i.invoice_no, i.date, i.customer_id, i.subtotal, i.cgst, i.sgst,
                    i.igst, i.grand_total, i.payment_type, i.sync_status, i.created_at,
                    i.created_by_user_id, c.name, c.mobile,
                    (SELECT COUNT(*) FROM invoice_lines l WHERE l.invoice_id = i.id),
                    u.display_name
               FROM invoices i
               JOIN customers c ON c.id = i.customer_id
               LEFT JOIN users u ON u.id = i.created_by_user_id
              WHERE (:from IS NULL OR i.date >= :from)
                AND (:to   IS NULL OR i.date <= :to)
                AND (:pattern IS NULL
                     OR c.name LIKE :pattern COLLATE NOCASE
                     OR i.invoice_no LIKE :pattern COLLATE NOCASE)
              ORDER BY i.date DESC, i.id DESC
              LIMIT :limit",
        )?;

        let rows = stmt.query_map(
            rusqlite::named_params! {
                ":from": filter.from.as_deref(),
                ":to": filter.to.as_deref(),
                ":pattern": pattern.as_deref(),
                ":limit": limit,
            },
            |row| {
                Ok(InvoiceSummary {
                    invoice: invoice_from_row(row)?,
                    customer_name: row.get(13)?,
                    customer_mobile: row.get(14)?,
                    line_count: row.get(15)?,
                    created_by: row.get(16)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// One saved invoice with its buyer and its lines, ready to display or reprint with
    /// no further lookups. Read-only — nothing here can change a stored invoice.
    pub fn get_invoice_detail(&self, id: i64) -> Result<Option<InvoiceDetail>> {
        let Some(invoice) = self.get_invoice(id)? else {
            return Ok(None);
        };
        let customer = self
            .get_customer(invoice.customer_id)?
            .ok_or_else(|| CoreError::NotFound(format!("customer {}", invoice.customer_id)))?;

        let mut stmt = self.conn.prepare(
            "SELECT l.id, l.invoice_id, l.item_id, l.qty, l.rate, l.tax_rate, l.line_total,
                    it.item_code, it.description, it.uom
               FROM invoice_lines l
               JOIN items it ON it.id = l.item_id
              WHERE l.invoice_id = ?1
              ORDER BY l.id",
        )?;
        let rows = stmt.query_map([id], |row| {
            Ok(InvoiceDetailLine {
                line: line_from_row(row)?,
                item_code: row.get(7)?,
                description: row.get(8)?,
                uom: row.get(9)?,
            })
        })?;

        let created_by = match invoice.created_by_user_id {
            Some(user_id) => self.get_user(user_id)?,
            None => None,
        };

        Ok(Some(InvoiceDetail {
            invoice,
            customer,
            lines: rows.collect::<rusqlite::Result<Vec<_>>>()?,
            created_by,
        }))
    }

    pub fn get_invoice(&self, id: i64) -> Result<Option<Invoice>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, invoice_no, date, customer_id, subtotal, cgst, sgst, igst,
                        grand_total, payment_type, sync_status, created_at,
                        created_by_user_id
                   FROM invoices WHERE id = ?1",
                [id],
                invoice_from_row,
            )
            .optional()?)
    }

    /// The lines of an invoice, in entry order.
    pub fn invoice_lines(&self, invoice_id: i64) -> Result<Vec<InvoiceLine>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, invoice_id, item_id, qty, rate, tax_rate, line_total
               FROM invoice_lines
              WHERE invoice_id = ?1
              ORDER BY id",
        )?;
        let rows = stmt.query_map([invoice_id], line_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ------------------------------------------------------------- credit notes

    /// What can still be credited on an invoice, line by line.
    ///
    /// Built from the invoice's own lines less whatever earlier notes already took, so a
    /// line credited twice at half quantity shows nothing left the third time. This is
    /// what the form is drawn from and what [`Db::create_credit_note`] validates against.
    pub fn creditable_lines(&self, invoice_id: i64) -> Result<Vec<CreditableLine>> {
        let mut stmt = self.conn.prepare(
            "SELECT l.id, l.item_id, i.item_code, i.description, l.rate, l.tax_rate, i.uom,
                    l.qty,
                    COALESCE((SELECT SUM(c.qty) FROM credit_note_lines c
                               WHERE c.invoice_line_id = l.id), 0)
               FROM invoice_lines l
               JOIN items i ON i.id = l.item_id
              WHERE l.invoice_id = ?1
              ORDER BY l.id",
        )?;
        let rows = stmt.query_map([invoice_id], |row| {
            let billed_qty: f64 = row.get(7)?;
            let credited_qty: f64 = row.get(8)?;
            Ok(CreditableLine {
                invoice_line_id: row.get(0)?,
                item_id: row.get(1)?,
                item_code: row.get(2)?,
                description: row.get(3)?,
                rate: row.get(4)?,
                tax_rate: row.get(5)?,
                uom: row.get(6)?,
                billed_qty,
                credited_qty,
                creditable_qty: (billed_qty - credited_qty).max(0.0),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Issues a credit note against an invoice.
    ///
    /// The invoice is read, never written. Nothing in this function touches `invoices` or
    /// `invoice_lines`: the original stays exactly as it was billed, because that is what
    /// the customer holds and what a return was filed on. The correction is a separate
    /// linked document that nets against it.
    ///
    /// Totals are recomputed from the credited lines by the same GST code that priced the
    /// invoice — never a proportion of the original. Crediting 3 of 7 units is three units
    /// priced and taxed, not three sevenths of a number, which is how rounding errors get
    /// into a tax return.
    pub fn create_credit_note(&mut self, new: &NewCreditNote) -> Result<CreditNote> {
        let reason = new.reason.trim().to_string();
        if reason.is_empty() {
            return Err(CoreError::Invalid("a reason is required".into()));
        }

        let invoice = self
            .get_invoice(new.original_invoice_id)?
            .ok_or_else(|| CoreError::NotFound(format!("invoice {}", new.original_invoice_id)))?;

        let date = match &new.date {
            Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .map_err(|_| CoreError::Invalid(format!("date must be YYYY-MM-DD, got {d}")))?,
            None => Local::now().date_naive(),
        };

        // What is left to credit, before anything is written.
        let creditable = self.creditable_lines(invoice.id)?;

        let mut priced: Vec<(CreditableLine, f64, TaxableLine)> = Vec::new();
        for line in &new.lines {
            if !line.qty.is_finite() || line.qty <= 0.0 {
                // A zero line is simply not credited; a negative one is a caller trying to
                // *add* to an invoice through the correction path, which is not what this
                // is for.
                if line.qty == 0.0 {
                    continue;
                }
                return Err(CoreError::Invalid(
                    "a credited quantity must be greater than zero".into(),
                ));
            }

            let source = creditable
                .iter()
                .find(|c| c.invoice_line_id == line.invoice_line_id)
                .ok_or_else(|| {
                    CoreError::Invalid(format!(
                        "line {} is not on invoice {}",
                        line.invoice_line_id, invoice.invoice_no
                    ))
                })?;

            // The ceiling is what is left, not what was billed. Two notes of half each
            // must not add up to more than the whole, however they are ordered.
            if line.qty > source.creditable_qty + QTY_EPSILON {
                return Err(CoreError::Invalid(format!(
                    "cannot credit {} of {} — only {} remain uncredited",
                    line.qty, source.item_code, source.creditable_qty
                )));
            }

            // Priced at what it sold for, from the invoice line, not from today's
            // catalogue: a price change after the sale must not change the refund.
            priced.push((
                source.clone(),
                line.qty,
                TaxableLine { qty: line.qty, rate: source.rate, tax_rate: source.tax_rate },
            ));
        }

        if priced.is_empty() {
            return Err(CoreError::Invalid("nothing to credit".into()));
        }

        // The same split the invoice carried. Deriving it from the invoice rather than
        // recomputing from the customer's state is deliberate: if a customer's place of
        // supply is edited after the sale, the credit must still reverse the tax that was
        // actually charged, not the tax that would be charged today.
        let taxables: Vec<TaxableLine> = priced.iter().map(|(_, _, t)| *t).collect();
        let totals = if invoice.igst > 0.0 {
            gst::compute_totals(&taxables, &self.home_state, INTER_STATE_PLACEHOLDER)
        } else {
            gst::compute_totals(&taxables, &self.home_state, &self.home_state)
        };

        // IMMEDIATE for the same reason invoices use it: the number is read and inserted
        // under one write lock, so two consoles cannot allocate the same one.
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let fy = financial_year(date);
        let credit_note_no = next_credit_note_no(fy, highest_credit_note_no(&tx, fy)?.as_deref())?;
        let created_at = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

        tx.execute(
            "INSERT INTO credit_notes
                 (credit_note_no, original_invoice_id, date, reason, subtotal, cgst, sgst,
                  igst, grand_total, sync_status, created_at, created_by_user_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', ?10, ?11)",
            params![
                credit_note_no,
                invoice.id,
                date.to_string(),
                reason,
                totals.subtotal,
                totals.cgst,
                totals.sgst,
                totals.igst,
                totals.grand_total,
                created_at,
                new.created_by_user_id,
            ],
        )?;
        let credit_note_id = tx.last_insert_rowid();

        let credit_note = CreditNote {
            id: credit_note_id,
            credit_note_no,
            original_invoice_id: invoice.id,
            date: date.to_string(),
            reason,
            subtotal: totals.subtotal,
            cgst: totals.cgst,
            sgst: totals.sgst,
            igst: totals.igst,
            grand_total: totals.grand_total,
            sync_status: "pending".to_string(),
            created_at,
            created_by_user_id: new.created_by_user_id,
        };

        // Queued before its lines, for the same reason invoices are: the worker sends in
        // queue order, and a line must not reach the back office before its parent.
        enqueue(&tx, "credit_notes", credit_note.id, SyncOp::Insert, &credit_note)?;

        for (source, qty, taxable) in &priced {
            let line_total = gst::round_money(qty * source.rate);
            tx.execute(
                "INSERT INTO credit_note_lines
                     (credit_note_id, invoice_line_id, item_id, qty, rate, tax_rate, line_total)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    credit_note_id,
                    source.invoice_line_id,
                    source.item_id,
                    qty,
                    source.rate,
                    taxable.tax_rate,
                    line_total,
                ],
            )?;
            let line = CreditNoteLine {
                id: tx.last_insert_rowid(),
                credit_note_id,
                invoice_line_id: source.invoice_line_id,
                item_id: source.item_id,
                qty: *qty,
                rate: source.rate,
                tax_rate: taxable.tax_rate,
                line_total,
            };
            enqueue(&tx, "credit_note_lines", line.id, SyncOp::Insert, &line)?;
        }

        tx.commit()?;
        Ok(credit_note)
    }

    /// Every credit note against an invoice, oldest first, with its lines.
    pub fn credit_notes_for_invoice(&self, invoice_id: i64) -> Result<Vec<CreditNoteDetail>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, credit_note_no, original_invoice_id, date, reason, subtotal, cgst,
                    sgst, igst, grand_total, sync_status, created_at, created_by_user_id
               FROM credit_notes
              WHERE original_invoice_id = ?1
              ORDER BY id",
        )?;
        let notes = stmt
            .query_map([invoice_id], credit_note_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut out = Vec::with_capacity(notes.len());
        for note in notes {
            let created_by = match note.created_by_user_id {
                Some(id) => self.get_user(id)?,
                None => None,
            };
            out.push(CreditNoteDetail {
                lines: self.credit_note_lines(note.id)?,
                created_by,
                credit_note: note,
            });
        }
        Ok(out)
    }

    /// One credit note's lines.
    pub fn credit_note_lines(&self, credit_note_id: i64) -> Result<Vec<CreditNoteLine>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, credit_note_id, invoice_line_id, item_id, qty, rate, tax_rate,
                    line_total
               FROM credit_note_lines
              WHERE credit_note_id = ?1
              ORDER BY id",
        )?;
        let rows = stmt.query_map([credit_note_id], |row| {
            Ok(CreditNoteLine {
                id: row.get(0)?,
                credit_note_id: row.get(1)?,
                invoice_line_id: row.get(2)?,
                item_id: row.get(3)?,
                qty: row.get(4)?,
                rate: row.get(5)?,
                tax_rate: row.get(6)?,
                line_total: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// What an invoice comes to once its credit notes are taken off.
    pub fn invoice_net(&self, invoice_id: i64) -> Result<InvoiceNet> {
        let invoice = self
            .get_invoice(invoice_id)?
            .ok_or_else(|| CoreError::NotFound(format!("invoice {invoice_id}")))?;

        let (note_count, credited_total, credited_subtotal, credited_tax) = self.conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(grand_total), 0),
                    COALESCE(SUM(subtotal), 0),
                    COALESCE(SUM(cgst + sgst + igst), 0)
               FROM credit_notes
              WHERE original_invoice_id = ?1",
            [invoice_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;

        Ok(InvoiceNet {
            credited_total: gst::round_money(credited_total),
            credited_subtotal: gst::round_money(credited_subtotal),
            credited_tax: gst::round_money(credited_tax),
            net_total: gst::round_money(invoice.grand_total - credited_total),
            note_count,
        })
    }

    // --------------------------------------------------------------- sync queue

    /// Queued rows nothing has sent yet, oldest first. Unbounded: used by tests and by
    /// anything that wants the whole backlog. The worker takes [`Db::next_sync_batch`].
    pub fn pending_sync_rows(&self) -> Result<Vec<SyncQueueRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, table_name, row_id, op, payload_json, created_at, synced_at
               FROM sync_queue
              WHERE synced_at IS NULL
              ORDER BY id",
        )?;
        let rows = stmt.query_map([], sync_row_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The next batch to push, oldest first, capped at `limit`.
    ///
    /// Oldest first is load-bearing rather than tidy: an invoice's lines are queued after
    /// the invoice, and a customer before the invoice that references it, so sending in
    /// insertion order means the far end never sees a row whose parent has not arrived.
    /// A batch that fails is retried as the same batch, so that order survives retries.
    pub fn next_sync_batch(&self, limit: usize) -> Result<Vec<SyncQueueRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, table_name, row_id, op, payload_json, created_at, synced_at
               FROM sync_queue
              WHERE synced_at IS NULL
              ORDER BY id
              LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], sync_row_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// How many rows are still waiting to be sent.
    pub fn pending_sync_count(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE synced_at IS NULL",
            [],
            |row| row.get(0),
        )?)
    }

    /// Marks rows as sent, in one transaction.
    ///
    /// Called only after the far end has answered 2xx. All or nothing: a partial mark
    /// would leave rows that were accepted looking unsent, and the next batch would send
    /// them again. Already-marked rows keep their original timestamp, so a duplicate call
    /// cannot rewrite history.
    pub fn mark_synced(&mut self, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }

        let stamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let tx = self.conn.transaction()?;
        let mut marked = 0;
        {
            let mut stmt = tx.prepare(
                "UPDATE sync_queue SET synced_at = ?1 WHERE id = ?2 AND synced_at IS NULL",
            )?;
            for id in ids {
                marked += stmt.execute(params![stamp, id])?;
            }
        }
        tx.commit()?;
        Ok(marked)
    }

    /// Queued rows for one table, oldest first.
    pub fn pending_sync_rows_for(&self, table_name: &str) -> Result<Vec<SyncQueueRow>> {
        Ok(self.pending_sync_rows()?.into_iter().filter(|r| r.table_name == table_name).collect())
    }

    // ---------------------------------------------------------------- internals

    /// Resolves a caller's line against the item master, applying rate and tax overrides.
    fn price_line(&self, line: &NewInvoiceLine) -> Result<(i64, TaxableLine)> {
        let item = self
            .get_item(line.item_id)?
            .ok_or_else(|| CoreError::NotFound(format!("item {}", line.item_id)))?;
        if line.qty <= 0.0 {
            return Err(CoreError::Invalid(format!("qty must be positive, got {}", line.qty)));
        }
        Ok((
            item.id,
            TaxableLine {
                qty: line.qty,
                rate: line.rate.unwrap_or(item.rate),
                tax_rate: line.tax_rate.unwrap_or(item.tax_rate),
            },
        ))
    }

    // ------------------------------------------------------------------ analytics

    /// Everything billed in `range`, added up by SQLite.
    ///
    /// The Analytics pane displays these; it does not compute them. That is the same rule
    /// the billing screen follows — a figure somebody might file a return against is
    /// produced once, in one place, for every runtime.
    pub fn sales_summary(&self, range: &DateRange) -> Result<SalesSummary> {
        let (from, to) = (range.from.clone(), range.to.clone());

        let (invoice_count, subtotal, cgst, sgst, igst, grand_total): (
            i64,
            f64,
            f64,
            f64,
            f64,
            f64,
        ) = self.conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(subtotal), 0),
                    COALESCE(SUM(cgst), 0),
                    COALESCE(SUM(sgst), 0),
                    COALESCE(SUM(igst), 0),
                    COALESCE(SUM(grand_total), 0)
               FROM invoices
              WHERE (?1 IS NULL OR date >= ?1)
                AND (?2 IS NULL OR date <= ?2)",
            params![from, to],
            |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
            },
        )?;

        // Credit notes are dated when they were issued, and counted in the range they fall
        // in — not pushed back to the invoice's date. A return in October reduces October,
        // which is the month somebody has to file.
        let (credit_note_count, credited_subtotal, credited_tax, credited_total): (
            i64,
            f64,
            f64,
            f64,
        ) = self.conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(subtotal), 0),
                    COALESCE(SUM(cgst + sgst + igst), 0),
                    COALESCE(SUM(grand_total), 0)
               FROM credit_notes
              WHERE (?1 IS NULL OR date >= ?1)
                AND (?2 IS NULL OR date <= ?2)",
            params![from, to],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;

        let tax_total = gst::round_money(cgst + sgst + igst);

        Ok(SalesSummary {
            invoice_count,
            subtotal,
            cgst,
            sgst,
            igst,
            tax_total,
            grand_total,
            credit_note_count,
            credited_subtotal: gst::round_money(credited_subtotal),
            credited_tax: gst::round_money(credited_tax),
            credited_total: gst::round_money(credited_total),
            net_subtotal: gst::round_money(subtotal - credited_subtotal),
            net_tax: gst::round_money(tax_total - credited_tax),
            net_total: gst::round_money(grand_total - credited_total),
        })
    }

    /// One row per day that had billing, oldest first. A day with no sales is absent
    /// rather than zero: whether a gap is a gap or a zero is the caller's question.
    pub fn daily_totals(&self, range: &DateRange) -> Result<Vec<DailyTotal>> {
        let (from, to) = (range.from.clone(), range.to.clone());
        // A day appears if it had billing *or* credits: a day whose only activity was a
        // return is a real day with a real negative net, and dropping it would hide it.
        let mut stmt = self.conn.prepare(
            "SELECT d.date,
                    COALESCE(i.n, 0),
                    COALESCE(i.cgst_sgst, 0),
                    COALESCE(i.igst, 0),
                    COALESCE(i.billed, 0),
                    COALESCE(c.credited, 0)
               FROM (SELECT date FROM invoices
                      UNION SELECT date FROM credit_notes) d
               LEFT JOIN (SELECT date,
                                 COUNT(*) AS n,
                                 SUM(cgst + sgst) AS cgst_sgst,
                                 SUM(igst) AS igst,
                                 SUM(grand_total) AS billed
                            FROM invoices GROUP BY date) i ON i.date = d.date
               LEFT JOIN (SELECT date, SUM(grand_total) AS credited
                            FROM credit_notes GROUP BY date) c ON c.date = d.date
              WHERE (?1 IS NULL OR d.date >= ?1)
                AND (?2 IS NULL OR d.date <= ?2)
              ORDER BY d.date",
        )?;
        let rows = stmt.query_map(params![from, to], |row| {
            let grand_total: f64 = row.get(4)?;
            let credited_total: f64 = row.get(5)?;
            Ok(DailyTotal {
                date: row.get(0)?,
                invoice_count: row.get(1)?,
                cgst_sgst: row.get(2)?,
                igst: row.get(3)?,
                grand_total,
                credited_total,
                net_total: grand_total - credited_total,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// What sold, by revenue, best first.
    pub fn top_items(&self, range: &DateRange, limit: i64) -> Result<Vec<TopItem>> {
        let (from, to) = (range.from.clone(), range.to.clone());
        // Ranked on revenue *after* credits. An item sold ten times and returned nine is
        // not this shop's best seller, and a list that said so would be worse than no
        // list.
        let mut stmt = self.conn.prepare(
            "SELECT i.item_code,
                    i.description,
                    COALESCE(s.qty, 0) - COALESCE(r.qty, 0) AS net_qty,
                    COALESCE(s.revenue, 0) - COALESCE(r.revenue, 0) AS net_revenue,
                    COALESCE(r.qty, 0),
                    COALESCE(r.revenue, 0)
               FROM items i
               LEFT JOIN (SELECT l.item_id,
                                 SUM(l.qty) AS qty,
                                 SUM(l.line_total) AS revenue
                            FROM invoice_lines l
                            JOIN invoices v ON v.id = l.invoice_id
                           WHERE (?1 IS NULL OR v.date >= ?1)
                             AND (?2 IS NULL OR v.date <= ?2)
                           GROUP BY l.item_id) s ON s.item_id = i.id
               LEFT JOIN (SELECT cl.item_id,
                                 SUM(cl.qty) AS qty,
                                 SUM(cl.line_total) AS revenue
                            FROM credit_note_lines cl
                            JOIN credit_notes cn ON cn.id = cl.credit_note_id
                           WHERE (?1 IS NULL OR cn.date >= ?1)
                             AND (?2 IS NULL OR cn.date <= ?2)
                           GROUP BY cl.item_id) r ON r.item_id = i.id
              WHERE s.item_id IS NOT NULL OR r.item_id IS NOT NULL
              ORDER BY net_revenue DESC
              LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![from, to, limit], |row| {
            Ok(TopItem {
                item_code: row.get(0)?,
                description: row.get(1)?,
                qty: row.get(2)?,
                revenue: row.get(3)?,
                credited_qty: row.get(4)?,
                credited_revenue: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// How customers paid, biggest share first.
    pub fn payment_mix(&self, range: &DateRange) -> Result<Vec<PaymentMix>> {
        let (from, to) = (range.from.clone(), range.to.clone());
        // Credits are attributed to the payment type of the invoice they reverse: money
        // refunded on a card sale did not arrive as cash, whatever the refund itself was.
        let mut stmt = self.conn.prepare(
            "SELECT v.payment_type,
                    COUNT(*),
                    COALESCE(SUM(v.grand_total), 0)
                      - COALESCE((SELECT SUM(cn.grand_total)
                                    FROM credit_notes cn
                                    JOIN invoices vi ON vi.id = cn.original_invoice_id
                                   WHERE vi.payment_type = v.payment_type
                                     AND (?1 IS NULL OR cn.date >= ?1)
                                     AND (?2 IS NULL OR cn.date <= ?2)), 0) AS billed
               FROM invoices v
              WHERE (?1 IS NULL OR v.date >= ?1)
                AND (?2 IS NULL OR v.date <= ?2)
              GROUP BY v.payment_type
              ORDER BY billed DESC",
        )?;
        let rows = stmt.query_map(params![from, to], |row| {
            Ok(PaymentMix {
                payment_type: row.get(0)?,
                invoice_count: row.get(1)?,
                grand_total: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// Floating-point slack when comparing a credited quantity against what remains.
///
/// Quantities are `REAL`, so 7.0 minus three lots of 2.333… does not land exactly on
/// zero. Without this, crediting the last sliver of a line would be refused for a
/// rounding artefact the person on the screen cannot see.
const QTY_EPSILON: f64 = 1e-9;

/// A place of supply that is deliberately not the home state, used to reproduce an
/// inter-state split on a credit note without re-reading the customer's current address.
const INTER_STATE_PLACEHOLDER: &str = "__INTER_STATE__";

/// Highest credit note number issued in a financial year, or `None` for a fresh year.
/// Lexical MAX works for the same reason it does for invoices: fixed prefix, zero-padded.
fn highest_credit_note_no(conn: &Connection, fy: i32) -> Result<Option<String>> {
    let prefix = format!("{}-{fy}-%", crate::numbering::CREDIT_NOTE_PREFIX);
    Ok(conn
        .query_row(
            "SELECT MAX(credit_note_no) FROM credit_notes WHERE credit_note_no LIKE ?1",
            [prefix],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

fn credit_note_from_row(row: &Row<'_>) -> rusqlite::Result<CreditNote> {
    Ok(CreditNote {
        id: row.get(0)?,
        credit_note_no: row.get(1)?,
        original_invoice_id: row.get(2)?,
        date: row.get(3)?,
        reason: row.get(4)?,
        subtotal: row.get(5)?,
        cgst: row.get(6)?,
        sgst: row.get(7)?,
        igst: row.get(8)?,
        grand_total: row.get(9)?,
        sync_status: row.get(10)?,
        created_at: row.get(11)?,
        created_by_user_id: row.get(12)?,
    })
}

/// Highest invoice number issued in a financial year, or `None` for a fresh year.
/// Lexical MAX works because the numbers are fixed-prefix and zero-padded. Takes a
/// connection rather than `&self` so it can be called from inside the transaction that
/// will insert the number it hands back.
fn highest_invoice_no(conn: &Connection, fy: i32) -> Result<Option<String>> {
    let prefix = format!("RI-{fy}-%");
    Ok(conn.query_row(
        "SELECT MAX(invoice_no) FROM invoices WHERE invoice_no LIKE ?1",
        [prefix],
        |row| row.get::<_, Option<String>>(0),
    )?)
}

/// Inserts one invoice line and queues it. Shared by `create_invoice` and `add_line_item`.
fn insert_line(
    tx: &Transaction<'_>,
    invoice_id: i64,
    item_id: i64,
    taxable: &TaxableLine,
) -> Result<i64> {
    let line_total = taxable.line_total();
    tx.execute(
        "INSERT INTO invoice_lines (invoice_id, item_id, qty, rate, tax_rate, line_total)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![invoice_id, item_id, taxable.qty, taxable.rate, taxable.tax_rate, line_total],
    )?;
    let id = tx.last_insert_rowid();

    let line = InvoiceLine {
        id,
        invoice_id,
        item_id,
        qty: taxable.qty,
        rate: taxable.rate,
        tax_rate: taxable.tax_rate,
        line_total,
    };
    enqueue(tx, "invoice_lines", id, SyncOp::Insert, &line)?;
    Ok(id)
}

/// Queues a change for a later stage to send. Always called inside the transaction that
/// makes the change it describes.
fn enqueue<T: Serialize>(
    tx: &Transaction<'_>,
    table_name: &str,
    row_id: i64,
    op: SyncOp,
    payload: &T,
) -> Result<()> {
    let payload_json = serde_json::to_string(payload)?;
    tx.execute(
        "INSERT INTO sync_queue (table_name, row_id, op, payload_json)
         VALUES (?1, ?2, ?3, ?4)",
        params![table_name, row_id, op.as_str(), payload_json],
    )?;
    Ok(())
}

/// Failed attempts inside the window before a username is shut out.
pub const MAX_FAILED_LOGINS: i64 = 5;

/// How long failures are remembered for, in minutes. Attempts age out of this window on
/// their own; nothing resets it early.
///
/// One minute is short for a lockout. It is enough to make guessing at bcrypt speed
/// pointless — five tries a minute is not a brute force — while keeping a mistyped
/// password from costing a counter its next customer. Raise it if this ever faces
/// anything but a shop floor.
pub const LOGIN_WINDOW_MINUTES: i64 = 1;

/// One spelling of a username, used by the credential check and the limiter alike.
///
/// Shared deliberately. If the two disagreed, "PRIYA" and "priya" would be one account to
/// sign in as and two buckets to count against, and five attempts each would be ten.
pub fn normalise_username(username: &str) -> String {
    username.trim().to_lowercase()
}

/// A real bcrypt hash of a value nobody knows, verified against when no such user
/// exists so that a missing username costs the same time as a wrong password.
const DUMMY_HASH: &str = "$2b$12$C6UzMDM.H6dfI/f/IKcEeODuLPFbCMovVlIVoiUCnLPRTOBzHfyOq";

fn user_from_row(row: &Row<'_>) -> rusqlite::Result<User> {
    let role: String = row.get(3)?;
    Ok(User {
        id: row.get(0)?,
        username: row.get(1)?,
        display_name: row.get(2)?,
        role: Role::parse(&role).unwrap_or(Role::Cashier),
        created_at: row.get(4)?,
    })
}

fn customer_from_row(row: &Row<'_>) -> rusqlite::Result<Customer> {
    Ok(Customer {
        id: row.get(0)?,
        name: row.get(1)?,
        gstin: row.get(2)?,
        place_of_supply: row.get(3)?,
        mobile: row.get(4)?,
    })
}

fn item_from_row(row: &Row<'_>) -> rusqlite::Result<Item> {
    Ok(Item {
        id: row.get(0)?,
        item_code: row.get(1)?,
        description: row.get(2)?,
        rate: row.get(3)?,
        tax_rate: row.get(4)?,
        uom: row.get(5)?,
    })
}

fn invoice_from_row(row: &Row<'_>) -> rusqlite::Result<Invoice> {
    Ok(Invoice {
        id: row.get(0)?,
        invoice_no: row.get(1)?,
        date: row.get(2)?,
        customer_id: row.get(3)?,
        subtotal: row.get(4)?,
        cgst: row.get(5)?,
        sgst: row.get(6)?,
        igst: row.get(7)?,
        grand_total: row.get(8)?,
        payment_type: row.get(9)?,
        sync_status: row.get(10)?,
        created_at: row.get(11)?,
        created_by_user_id: row.get(12)?,
    })
}

fn line_from_row(row: &Row<'_>) -> rusqlite::Result<InvoiceLine> {
    Ok(InvoiceLine {
        id: row.get(0)?,
        invoice_id: row.get(1)?,
        item_id: row.get(2)?,
        qty: row.get(3)?,
        rate: row.get(4)?,
        tax_rate: row.get(5)?,
        line_total: row.get(6)?,
    })
}

fn sync_row_from_row(row: &Row<'_>) -> rusqlite::Result<SyncQueueRow> {
    Ok(SyncQueueRow {
        id: row.get(0)?,
        table_name: row.get(1)?,
        row_id: row.get(2)?,
        op: row.get(3)?,
        payload_json: row.get(4)?,
        created_at: row.get(5)?,
        synced_at: row.get(6)?,
    })
}
