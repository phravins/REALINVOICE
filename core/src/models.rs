use serde::{Deserialize, Serialize};

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
