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
use crate::numbering::{financial_year, next_invoice_no};
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
    pub fn verify_login(&self, username: &str, password: &str) -> Result<Option<User>> {
        let username = username.trim().to_lowercase();
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

        for (item_id, taxable) in &priced {
            insert_line(&tx, invoice_id, *item_id, taxable)?;
        }

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
        enqueue(&tx, "invoices", invoice.id, SyncOp::Insert, &invoice)?;
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

    // --------------------------------------------------------------- sync queue

    /// Queued rows nothing has sent yet, oldest first. No consumer exists yet; this is
    /// here so tests — and, later, the sync worker — can see what is waiting.
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
