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
    /// Which price list this buyer is billed from. `None` means whichever list is
    /// currently default — not "no pricing", and deliberately not a copy of today's
    /// default id, so a shop that renames or re-points its default does not leave every
    /// existing customer pinned to the old one.
    #[serde(default)]
    pub price_list_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewCustomer {
    pub name: String,
    pub gstin: Option<String>,
    pub place_of_supply: String,
    pub mobile: String,
    #[serde(default)]
    pub price_list_id: Option<i64>,
}

/// A sellable line item. `tax_rate` is the total GST percentage (e.g. 18.0), which the
/// GST module splits in half for intra-state sales.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: i64,
    pub item_code: String,
    pub description: String,
    /// The item's **base** rate, and the fallback for any price list with no entry for
    /// it. This is not necessarily what a given customer is billed — see
    /// [`crate::Db::resolve_rate`], which is the only thing billing should ask.
    pub rate: f64,
    pub tax_rate: f64,
    pub uom: String,
    /// Billed once from the billing screen rather than kept in the price list. Hidden
    /// from the Inventory list and from the item picker, and tagged on the invoice so it
    /// is clear at a glance which lines came from the catalogue.
    #[serde(default)]
    pub custom: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewItem {
    pub item_code: String,
    pub description: String,
    pub rate: f64,
    pub tax_rate: f64,
    pub uom: String,
    #[serde(default)]
    pub custom: bool,
}

/// How a discount is expressed.
///
/// There is no "after tax" variant, and that is a deliberate omission rather than a gap.
/// A discount known and disclosed when the sale happens is a trade discount: it reduces
/// the taxable value, and GST is charged on what was actually charged. A discount handed
/// over after the invoice is a legally distinct thing that generally cannot reduce GST
/// liability retrospectively. Offering both as a toggle would let a shop pick the wrong
/// one by accident, so this type only expresses the correct one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscountType {
    #[default]
    None,
    /// `value` is a percentage of the amount being discounted, e.g. 10.0 for 10%.
    Percentage,
    /// `value` is rupees off, e.g. 500.0.
    Flat,
}

impl DiscountType {
    pub fn as_str(self) -> &'static str {
        match self {
            DiscountType::None => "none",
            DiscountType::Percentage => "percentage",
            DiscountType::Flat => "flat",
        }
    }

    /// Anything unrecognised reads as no discount. A row whose type could not be parsed
    /// must not silently become a percentage.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(DiscountType::None),
            "percentage" => Some(DiscountType::Percentage),
            "flat" => Some(DiscountType::Flat),
            _ => None,
        }
    }
}

impl std::fmt::Display for DiscountType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A named set of prices — "Retail", "Wholesale", "VIP".
///
/// Exactly one list is the default at any time, enforced by a partial unique index rather
/// than by convention. The default is what a customer with no list of their own is billed
/// from, and what the billing screen assumes before anyone is attached.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceList {
    pub id: i64,
    pub name: String,
    pub is_default: bool,
    pub created_at: String,
}

/// One item's rate on one list. Absent rather than zero when a list does not price an
/// item: a missing entry falls back to the item's base rate, where a zero would mean the
/// shop is giving it away.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemPrice {
    pub item_id: i64,
    pub price_list_id: i64,
    pub rate: f64,
}

/// What the item edit form draws: every list, and this item's rate on it if it has one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemPriceRow {
    pub price_list: PriceList,
    /// `None` where the list has no entry and the base rate applies.
    pub rate: Option<f64>,
}

/// The answer to "what does this customer pay for this item".
///
/// Carries where the number came from as well as the number itself, so a screen can say
/// "this is the Wholesale rate" rather than leaving a cashier to wonder why the total
/// differs from the sticker price.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ResolvedRate {
    pub item_id: i64,
    pub price_list_id: i64,
    pub rate: f64,
    /// False when the list had no entry for this item and its base rate was used.
    pub from_price_list: bool,
}

/// A catalogue item with its rate already resolved for one price list.
///
/// The picker shows these rather than bare [`Item`]s so the rate on screen is the rate
/// that will be billed, not the base rate that may or may not be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricedItem {
    pub item: Item,
    pub rate: f64,
    pub from_price_list: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invoice {
    pub id: i64,
    pub invoice_no: String,
    /// ISO-8601 date, `YYYY-MM-DD`.
    pub date: String,
    pub customer_id: i64,
    /// What the bill would have come to before any discount: the sum of every line's
    /// `qty * rate`. Stored rather than derived, so a later rate change cannot move it.
    #[serde(default)]
    pub pre_discount_subtotal: f64,
    /// Line discounts plus the invoice discount, in rupees.
    #[serde(default)]
    pub discount_amount: f64,
    #[serde(default)]
    pub invoice_discount_type: DiscountType,
    #[serde(default)]
    pub invoice_discount_value: f64,
    /// The invoice-level discount alone, in rupees.
    #[serde(default)]
    pub invoice_discount_amount: f64,
    /// **The taxable value** — what is left after every discount, and what CGST/SGST/IGST
    /// were charged on. Equal to `pre_discount_subtotal` when nothing was discounted.
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
    /// True for a one-off item typed straight onto the bill. Carried through so a
    /// reprint and the history view can mark the line the same way the billing card
    /// did — the code on a one-off is a synthetic key, not something to read out.
    #[serde(default)]
    pub custom: bool,
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
    /// `qty * rate`, before any discount. Unchanged in meaning from before discounts
    /// existed, which is why every historical line is still correct.
    pub line_total: f64,
    #[serde(default)]
    pub discount_type: DiscountType,
    #[serde(default)]
    pub discount_value: f64,
    /// This line's own discount, in rupees.
    #[serde(default)]
    pub discount_amount: f64,
    /// This line's apportioned share of the invoice-level discount, in rupees. Part of
    /// the tax base, so it is stored per line rather than left to be re-derived.
    #[serde(default)]
    pub invoice_discount_share: f64,
    /// `line_total - discount_amount - invoice_discount_share`: what tax was charged on.
    #[serde(default)]
    pub taxable_value: f64,
}

impl InvoiceLine {
    /// What this line actually cost, tax included. Derived from the stored taxable value
    /// and nothing else, so it says the same thing in ten years' time.
    pub fn charged(&self, intra_state: bool) -> f64 {
        let tax = crate::gst::split_line_tax(self.taxable_value, self.tax_rate, intra_state);
        crate::gst::round_money(self.taxable_value + tax.total())
    }
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
    /// A trade discount on this line, off `qty * rate` before tax.
    #[serde(default)]
    pub discount_type: DiscountType,
    #[serde(default)]
    pub discount_value: f64,
}

/// What a caller hands to [`crate::Db::create_invoice`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewInvoice {
    pub customer_id: i64,
    /// Defaults to today when absent.
    #[serde(default)]
    pub date: Option<String>,
    pub payment_type: String,
    /// A trade discount on the whole bill, applied to the subtotal that is left *after*
    /// every line discount — so 10% off is 10% of what the lines already came down to.
    #[serde(default)]
    pub invoice_discount_type: DiscountType,
    #[serde(default)]
    pub invoice_discount_value: f64,
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

/// What the Inventory pane filters the catalogue by.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ItemFilter {
    /// Substring matched against item code or description.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// A date range for the Analytics pane. Both bounds are inclusive `YYYY-MM-DD`, and
/// either may be open.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DateRange {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

/// Everything billed in a range, added up.
///
/// Computed in SQL, like every other figure in this product. No total in this app is ever
/// reached by adding numbers up in JavaScript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SalesSummary {
    pub invoice_count: i64,
    pub subtotal: f64,
    pub cgst: f64,
    pub sgst: f64,
    pub igst: f64,
    /// `cgst + sgst + igst` — what has to be remitted, whichever way it split.
    pub tax_total: f64,
    pub grand_total: f64,

    /// Credit notes issued in the same range.
    ///
    /// Kept beside the gross figures rather than folded into them, because both are real:
    /// what was billed is what the invoices say, and what was earned is what is left after
    /// corrections. A report that showed only one would be answering a different question
    /// from the one asked.
    pub credit_note_count: i64,
    pub credited_subtotal: f64,
    pub credited_tax: f64,
    pub credited_total: f64,

    /// What the shop actually earned: billed less credited. This is the figure to file a
    /// return against, and the one the Analytics pane leads with.
    pub net_subtotal: f64,
    pub net_tax: f64,
    pub net_total: f64,
}

/// One day's billing, for the chart across the top of Analytics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailyTotal {
    pub date: String,
    pub invoice_count: i64,
    pub cgst_sgst: f64,
    pub igst: f64,
    pub grand_total: f64,
    /// Credited on the same day. A day with a large return can net negative, which is
    /// true and worth seeing rather than clamping to zero.
    pub credited_total: f64,
    pub net_total: f64,
}

/// What sold, by revenue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopItem {
    pub item_code: String,
    pub description: String,
    /// Quantity billed less quantity credited back.
    pub qty: f64,
    /// Revenue net of credits, which is what "what sold" means once returns exist.
    pub revenue: f64,
    pub credited_qty: f64,
    pub credited_revenue: f64,
}

/// How customers paid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaymentMix {
    pub payment_type: String,
    pub invoice_count: i64,
    /// Net of credit notes against invoices taken that way.
    pub grand_total: f64,
}

/// What a sign-in attempt came to.
///
/// A locked-out attempt is deliberately its own answer rather than an error: the screen
/// has to say something different, and a caller that cannot tell the two apart would show
/// "wrong password" to somebody whose password is right.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LoginOutcome {
    /// Credentials matched.
    Ok(User),
    /// No such user, or the wrong password. One answer for both, so the screen cannot be
    /// used to find out which accounts exist.
    Invalid,
    /// Too many recent failures for this username. The password was not checked.
    LockedOut(Lockout),
}

/// How long a username is shut out for, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lockout {
    /// Failed attempts inside the window.
    pub failures: i64,
    /// How many more are allowed before a lockout, which is zero here by definition.
    /// Present so the screen can warn on the way to a lockout as well as after one.
    pub remaining: i64,
    /// Seconds until the oldest failure ages out and an attempt is allowed again.
    pub retry_after_seconds: i64,
}

/// One recorded sign-in attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginAttempt {
    pub id: i64,
    pub username: String,
    pub attempted_at: String,
    pub success: bool,
    /// Turned away by the lockout without the password being checked. Logged, but not
    /// counted towards the limit.
    pub refused: bool,
}

// ------------------------------------------------------------------ credit notes

/// A correction against an invoice. The invoice itself is never touched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditNote {
    pub id: i64,
    pub credit_note_no: String,
    pub original_invoice_id: i64,
    pub date: String,
    pub reason: String,
    pub subtotal: f64,
    pub cgst: f64,
    pub sgst: f64,
    pub igst: f64,
    pub grand_total: f64,
    pub sync_status: String,
    pub created_at: String,
    pub created_by_user_id: Option<i64>,
}

/// One credited line, tied to the invoice line it reverses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditNoteLine {
    pub id: i64,
    pub credit_note_id: i64,
    pub invoice_line_id: i64,
    pub item_id: i64,
    pub qty: f64,
    pub rate: f64,
    pub tax_rate: f64,
    pub line_total: f64,
}

/// What a caller asks for. Attribution is filled from the session by the runtime, never
/// taken from the payload — the same rule invoices follow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewCreditNote {
    pub original_invoice_id: i64,
    /// Free text, required. Core refuses a blank one.
    pub reason: String,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub created_by_user_id: Option<i64>,
    pub lines: Vec<NewCreditNoteLine>,
}

/// One line to credit, by invoice line and quantity. A line credited at zero is simply
/// left out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewCreditNoteLine {
    pub invoice_line_id: i64,
    pub qty: f64,
}

/// A credit note with its lines, for display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditNoteDetail {
    pub credit_note: CreditNote,
    pub lines: Vec<CreditNoteLine>,
    /// Who issued it, if the record still names a user on this machine.
    pub created_by: Option<User>,
}

/// How much of one invoice line has already been credited, and how much is left.
///
/// What the form is built from and what the validation checks against: a line can be
/// credited across several notes, and the sum of them must never exceed what was billed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditableLine {
    pub invoice_line_id: i64,
    pub item_id: i64,
    pub item_code: String,
    pub description: String,
    /// The rate the line was billed at, before any discount. What the customer's copy
    /// shows in the Rate column.
    pub rate: f64,
    /// What each unit actually cost once every discount had come off:
    /// `taxable_value / billed_qty`. **This is what a credit is priced at** — refunding
    /// the sticker price on a discounted line would hand back money that was never taken.
    #[serde(default)]
    pub effective_rate: f64,
    pub tax_rate: f64,
    pub uom: String,
    /// Quantity on the original invoice line.
    pub billed_qty: f64,
    /// Already credited by earlier notes against this invoice.
    pub credited_qty: f64,
    /// `billed_qty - credited_qty`. Zero means this line is fully reversed already.
    pub creditable_qty: f64,
}

/// An invoice, what has been credited against it, and what that leaves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceNet {
    /// Total of every credit note against this invoice.
    pub credited_total: f64,
    pub credited_subtotal: f64,
    pub credited_tax: f64,
    /// The invoice's grand total less everything credited. Never below zero, because core
    /// refuses to credit more than was billed.
    pub net_total: f64,
    pub note_count: i64,
}

/// What `clear_demo_data` did.
///
/// The "kept" counts are the point: a demo row that has been billed is left in place, and
/// the screen says so rather than claiming a clean sweep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DemoDataCleared {
    pub items_removed: usize,
    /// Demo items left because an invoice or credit note still refers to them.
    pub items_kept: usize,
    pub customers_removed: usize,
    /// Demo customers left because they have been billed.
    pub customers_kept: usize,
}
