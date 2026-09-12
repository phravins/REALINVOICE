use serde::{Deserialize, Serialize};

/// Who is signed in at the counter.
///
/// Deliberately carries no `password_hash`: this struct crosses into the frontend, and a
/// hash that never leaves the database cannot leak from a UI bug.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub role: Role,
    pub created_at: String,
}

/// Two roles for now. `Owner` is the account seeded on first run; `Cashier` is everyone
/// else. Nothing branches on this yet beyond display — permissions come with the user
/// management screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Cashier,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Cashier => "cashier",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Role::Owner),
            "cashier" => Some(Role::Cashier),
            _ => None,
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A user to create. The password is passed separately to [`crate::Db::create_user`] so
/// it is never a field on something that might be logged or serialized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewUser {
    pub username: String,
    pub display_name: String,
    pub role: Role,
}

/// A buyer. `place_of_supply` is the two-letter state code (e.g. "TN") and decides
/// whether a sale is intra-state (CGST + SGST) or inter-state (IGST).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Customer {
    pub id: i64,
    pub name: String,
    pub gstin: Option<String>,
    pub place_of_supply: String,
    pub mobile: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewCustomer {
    pub name: String,
    pub gstin: Option<String>,
    pub place_of_supply: String,
    pub mobile: String,
}

/// A sellable line item. `tax_rate` is the total GST percentage (e.g. 18.0), which the
/// GST module splits in half for intra-state sales.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: i64,
    pub item_code: String,
    pub description: String,
    pub rate: f64,
    pub tax_rate: f64,
    pub uom: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewItem {
    pub item_code: String,
    pub description: String,
    pub rate: f64,
    pub tax_rate: f64,
    pub uom: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invoice {
    pub id: i64,
    pub invoice_no: String,
    /// ISO-8601 date, `YYYY-MM-DD`.
    pub date: String,
    pub customer_id: i64,
    pub subtotal: f64,
    pub cgst: f64,
    pub sgst: f64,
    pub igst: f64,
    pub grand_total: f64,
    pub payment_type: String,
    pub sync_status: String,
    /// Local wall-clock time the invoice was raised, `YYYY-MM-DD HH:MM:SS`. Local rather
    /// than UTC so it agrees with `date`, which is the counter's own day.
    pub created_at: String,
    /// The signed-in user who billed it. `None` for invoices raised before sign-in
    /// existed.
    pub created_by_user_id: Option<i64>,
}

/// An invoice as the history list shows it: the invoice plus who it was billed to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceSummary {
    pub invoice: Invoice,
    pub customer_name: String,
    pub customer_mobile: String,
    pub line_count: i64,
    /// Display name of whoever billed it, when it was attributed.
    pub created_by: Option<String>,
}

/// A saved invoice with everything needed to display or reprint it, with no further
/// lookups. Read-only: invoices are append-only once saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceDetail {
    pub invoice: Invoice,
    pub customer: Customer,
    pub lines: Vec<InvoiceDetailLine>,
    /// The user who billed it, if the invoice was attributed.
    pub created_by: Option<User>,
}

/// A stored line with its item's code and description resolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceDetailLine {
    pub line: InvoiceLine,
    pub item_code: String,
    pub description: String,
    pub uom: String,
}

/// What the history pane filters by. Every field is optional; an empty filter lists
/// everything, newest first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InvoiceFilter {
    /// Inclusive lower bound on `invoices.date`, `YYYY-MM-DD`.
    #[serde(default)]
    pub from: Option<String>,
    /// Inclusive upper bound on `invoices.date`.
    #[serde(default)]
    pub to: Option<String>,
    /// Substring matched against customer name or invoice number.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceLine {
    pub id: i64,
    pub invoice_id: i64,
    pub item_id: i64,
    pub qty: f64,
    pub rate: f64,
    pub tax_rate: f64,
    pub line_total: f64,
}

/// A line as supplied by a caller, before it has an id. `rate` and `tax_rate` are
/// optional: left out, they are taken from the item master.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewInvoiceLine {
    pub item_id: i64,
    pub qty: f64,
    #[serde(default)]
    pub rate: Option<f64>,
    #[serde(default)]
    pub tax_rate: Option<f64>,
}

/// What a caller hands to [`crate::Db::create_invoice`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewInvoice {
    pub customer_id: i64,
    /// Defaults to today when absent.
    #[serde(default)]
    pub date: Option<String>,
    pub payment_type: String,
    /// Who billed it, from the active session. `None` only where no one is signed in,
    /// which the desktop app never allows.
    #[serde(default)]
    pub created_by_user_id: Option<i64>,
    #[serde(default)]
    pub lines: Vec<NewInvoiceLine>,
}

/// One row of the outbound queue. Written inside the same transaction as the change it
/// describes; nothing drains it yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncQueueRow {
    pub id: i64,
    pub table_name: String,
    pub row_id: i64,
    pub op: String,
    pub payload_json: String,
    pub created_at: String,
    pub synced_at: Option<String>,
}

/// Insert vs. update, as recorded in `sync_queue.op`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncOp {
    Insert,
    Update,
}

impl SyncOp {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncOp::Insert => "insert",
            SyncOp::Update => "update",
        }
    }
}
