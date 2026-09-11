//! The `invoke()` surface.
//!
//! Every command here is a thin wrapper: lock the shared [`crate::state::AppState`], call
//! one core function, hand the result back. No billing logic lives in this crate — GST,
//! numbering and the `sync_queue` writes all belong to `realinvoice-core`, so the Phoenix
//! and Ratatui runtimes get identical behaviour for free.

use realinvoice_core::{Customer, Invoice, InvoiceLine, Item, NewInvoice, NewInvoiceLine};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::{AppState, NODE_NAME};

/// What the shell reports about itself: node name, connectivity, and where its data
/// lives. `connected` is a stub until the sync stage gives it something real to say.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatus {
    pub node: String,
    pub connected: bool,
    pub db_path: String,
}

/// An invoice as the frontend posts it. Mirrors core's `NewInvoice`, kept as its own type
/// so the wire shape can evolve without core's API moving.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewInvoicePayload {
    pub customer_id: i64,
    /// `YYYY-MM-DD`; today when omitted.
    #[serde(default)]
    pub date: Option<String>,
    pub payment_type: String,
    #[serde(default)]
    pub lines: Vec<NewLinePayload>,
}

/// One line of a posted invoice. `rate` and `tax_rate` fall back to the item master.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewLinePayload {
    pub item_id: i64,
    pub qty: f64,
    #[serde(default)]
    pub rate: Option<f64>,
    #[serde(default)]
    pub tax_rate: Option<f64>,
}

impl From<NewLinePayload> for NewInvoiceLine {
    fn from(line: NewLinePayload) -> Self {
        NewInvoiceLine {
            item_id: line.item_id,
            qty: line.qty,
            rate: line.rate,
            tax_rate: line.tax_rate,
        }
    }
}

impl From<NewInvoicePayload> for NewInvoice {
    fn from(payload: NewInvoicePayload) -> Self {
        NewInvoice {
            customer_id: payload.customer_id,
            date: payload.date,
            payment_type: payload.payment_type,
            lines: payload.lines.into_iter().map(Into::into).collect(),
        }
    }
}

/// Title-bar state for the shell.
#[tauri::command]
pub fn node_status(state: State<'_, AppState>) -> NodeStatus {
    NodeStatus {
        node: NODE_NAME.to_string(),
        // Stub: real connectivity arrives with the sync stage.
        connected: true,
        db_path: state.db_path().display().to_string(),
    }
}

/// Finds a customer by exact mobile number. `None` when nobody matches.
#[tauri::command]
pub fn search_customer(
    mobile: String,
    state: State<'_, AppState>,
) -> Result<Option<Customer>, String> {
    state.db().search_customer(&mobile).map_err(|e| e.to_string())
}

/// Substring search over item code and description.
#[tauri::command]
pub fn search_item(query: String, state: State<'_, AppState>) -> Result<Vec<Item>, String> {
    state.db().search_item(&query).map_err(|e| e.to_string())
}

/// Creates an invoice with its lines. Core allocates the number, splits GST and queues
/// the sync rows.
#[tauri::command]
pub fn create_invoice(
    payload: NewInvoicePayload,
    state: State<'_, AppState>,
) -> Result<Invoice, String> {
    state.db().create_invoice(&NewInvoice::from(payload)).map_err(|e| e.to_string())
}

/// Appends a line to an existing invoice and re-totals it.
#[tauri::command]
pub fn add_line_item(
    invoice_id: i64,
    line: NewLinePayload,
    state: State<'_, AppState>,
) -> Result<InvoiceLine, String> {
    state.db().add_line_item(invoice_id, &NewInvoiceLine::from(line)).map_err(|e| e.to_string())
}

/// Today's invoices, newest first. An empty list on a quiet morning — not an error.
#[tauri::command]
pub fn list_todays_invoices(state: State<'_, AppState>) -> Result<Vec<Invoice>, String> {
    state.db().list_todays_invoices().map_err(|e| e.to_string())
}
