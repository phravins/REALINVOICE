//! The `invoke()` surface.
//!
//! Every command here is a thin wrapper: lock the shared [`crate::state::AppState`], call
//! one core function, hand the result back. No billing logic lives in this crate — GST,
//! numbering and the `sync_queue` writes all belong to `realinvoice-core`, so the Phoenix
//! and Ratatui runtimes get identical behaviour for free.

use realinvoice_core::{
    gst, Customer, Invoice, InvoiceLine, Item, NewCustomer, NewInvoice, NewInvoiceLine,
};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::{AppState, NODE_NAME};

/// A customer as the New-customer form posts it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCustomerPayload {
    pub name: String,
    #[serde(default)]
    pub gstin: Option<String>,
    pub place_of_supply: String,
    pub mobile: String,
}

impl From<NewCustomerPayload> for NewCustomer {
    fn from(payload: NewCustomerPayload) -> Self {
        NewCustomer {
            name: payload.name,
            gstin: payload.gstin,
            place_of_supply: payload.place_of_supply,
            mobile: payload.mobile,
        }
    }
}

/// One row of the billing table, as priced on screen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteLinePayload {
    pub qty: f64,
    pub rate: f64,
    pub tax_rate: f64,
}

/// What the summary panel renders. Every number here comes out of core's GST module, so
/// the figures on screen are the ones that will be persisted — the frontend does no
/// arithmetic of its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvoiceQuote {
    /// Seller's state, for the intra/inter-state decision.
    pub home_state: String,
    /// True when the split is CGST + SGST rather than IGST.
    pub intra_state: bool,
    /// Taxable value per row, in the order supplied — the table's Total column.
    pub line_totals: Vec<f64>,
    pub subtotal: f64,
    pub cgst: f64,
    pub sgst: f64,
    pub igst: f64,
    pub grand_total: f64,
}

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

/// Registers a customer the counter could not resolve. Errors if the mobile number is
/// already on file.
#[tauri::command]
pub fn create_customer(
    payload: NewCustomerPayload,
    state: State<'_, AppState>,
) -> Result<Customer, String> {
    state.db().create_customer(&NewCustomer::from(payload)).map_err(|e| e.to_string())
}

/// Prices the rows currently on screen against a place of supply, without touching the
/// database. Pure and cheap, so the summary panel can re-quote on every edit; it exists
/// so the live totals and the saved invoice come from one implementation.
#[tauri::command]
pub fn quote_invoice(place_of_supply: String, lines: Vec<QuoteLinePayload>) -> InvoiceQuote {
    let home_state = realinvoice_core::DEFAULT_HOME_STATE;
    let taxable: Vec<gst::TaxableLine> = lines
        .iter()
        .map(|l| gst::TaxableLine { qty: l.qty, rate: l.rate, tax_rate: l.tax_rate })
        .collect();

    let totals = gst::compute_totals(&taxable, home_state, &place_of_supply);
    InvoiceQuote {
        home_state: home_state.to_string(),
        intra_state: gst::is_intra_state(home_state, &place_of_supply),
        line_totals: taxable.iter().map(|l| l.line_total()).collect(),
        subtotal: totals.subtotal,
        cgst: totals.cgst,
        sgst: totals.sgst,
        igst: totals.igst,
        grand_total: totals.grand_total,
    }
}
