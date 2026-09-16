//! # realinvoice-core
//!
//! The shared heart of RealInvoice: the SQLite schema, the GST rules, invoice numbering,
//! and the [`Db`] handle every runtime goes through. The Office Console (Tauri), the
//! Back-Office Web (Phoenix) and the Logistics Desk (Ratatui) all sit on top of this —
//! none of them reimplement billing logic.
//!
//! ```no_run
//! use realinvoice_core::{Db, DiscountType, NewInvoice, NewInvoiceLine};
//!
//! let mut db = Db::open("db.sqlite")?;
//! let customer = db.search_customer("9840012345")?.expect("walk-in not registered");
//! let invoice = db.create_invoice(&NewInvoice {
//!     customer_id: customer.id,
//!     date: None,
//!     payment_type: "cash".into(),
//!     created_by_user_id: None,
//!     // No rate: the price comes from the customer's price list. No discount either —
//!     // both are optional, and both are core's to resolve rather than the caller's.
//!     invoice_discount_type: DiscountType::None,
//!     invoice_discount_value: 0.0,
//!     lines: vec![NewInvoiceLine {
//!         item_id: 1,
//!         qty: 2.0,
//!         rate: None,
//!         tax_rate: None,
//!         discount_type: DiscountType::None,
//!         discount_value: 0.0,
//!     }],
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
pub mod sync;

pub use db::{
    normalise_username, Db, DEFAULT_DISCOUNT_APPROVAL_PCT, LOGIN_WINDOW_MINUTES, MAX_FAILED_LOGINS,
};
pub use error::{CoreError, Result};
pub use gst::{
    compute_discounted_totals, compute_totals, DiscountedLine, DiscountedTotals, InvoiceTotals,
    PricedLine, TaxSplit, TaxableLine, DEFAULT_HOME_STATE,
};
pub use models::*;
pub use numbering::{financial_year, format_invoice_no, next_invoice_no};
pub use sync::{SyncBatch, SyncConfig, SyncEnvelopeRow, SyncHandle, SyncStatus};
