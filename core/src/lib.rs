//! # realinvoice-core
//!
//! The shared heart of RealInvoice: the SQLite schema, the GST rules, invoice numbering,
//! and the [`Db`] handle every runtime goes through. The Office Console (Tauri), the
//! Back-Office Web (Phoenix) and the Logistics Desk (Ratatui) all sit on top of this —
//! none of them reimplement billing logic.
//!
//! ```no_run
//! use realinvoice_core::{Db, NewInvoice, NewInvoiceLine};
//!
//! let mut db = Db::open("db.sqlite")?;
//! let customer = db.search_customer("9840012345")?.expect("walk-in not registered");
//! let invoice = db.create_invoice(&NewInvoice {
//!     customer_id: customer.id,
//!     date: None,
//!     payment_type: "cash".into(),
//!     created_by_user_id: None,
//!     lines: vec![NewInvoiceLine { item_id: 1, qty: 2.0, rate: None, tax_rate: None }],
//! })?;
//! println!("{} = {}", invoice.invoice_no, invoice.grand_total);
//! # Ok::<(), realinvoice_core::CoreError>(())
//! ```

pub mod auth;
pub mod db;
pub mod error;
pub mod gst;
pub mod models;
pub mod numbering;
pub mod schema;
pub mod seed;

pub use db::Db;
pub use error::{CoreError, Result};
pub use gst::{compute_totals, InvoiceTotals, TaxSplit, TaxableLine, DEFAULT_HOME_STATE};
pub use models::*;
pub use numbering::{financial_year, format_invoice_no, next_invoice_no};
