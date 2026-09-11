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
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::Serialize;

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

        let fy = financial_year(date);
        let highest = self.highest_invoice_no(fy)?;
        let invoice_no = next_invoice_no(fy, highest.as_deref())?;

        let taxables: Vec<TaxableLine> =
            priced.iter().map(|(_, t): &(i64, TaxableLine)| *t).collect();
        let totals = gst::compute_totals(&taxables, &self.home_state, &customer.place_of_supply);

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO invoices
                 (invoice_no, date, customer_id, subtotal, cgst, sgst, igst,
                  grand_total, payment_type, sync_status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending')",
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
                    grand_total, payment_type, sync_status
               FROM invoices
              WHERE date = ?1
              ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([date.to_string()], invoice_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_invoice(&self, id: i64) -> Result<Option<Invoice>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, invoice_no, date, customer_id, subtotal, cgst, sgst, igst,
                        grand_total, payment_type, sync_status
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

    /// Highest invoice number issued in a financial year, or `None` for a fresh year.
    /// Lexical MAX works because the numbers are fixed-prefix and zero-padded.
    fn highest_invoice_no(&self, fy: i32) -> Result<Option<String>> {
        let prefix = format!("RI-{fy}-%");
        Ok(self.conn.query_row(
            "SELECT MAX(invoice_no) FROM invoices WHERE invoice_no LIKE ?1",
            [prefix],
            |row| row.get::<_, Option<String>>(0),
        )?)
    }
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
