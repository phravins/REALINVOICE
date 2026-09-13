//! The `invoke()` surface.
//!
//! Every command here is a thin wrapper: lock the shared [`crate::state::AppState`], call
//! one core function, hand the result back. No billing logic lives in this crate — GST,
//! numbering and the `sync_queue` writes all belong to `realinvoice-core`, so the Phoenix
//! and Ratatui runtimes get identical behaviour for free.

use realinvoice_core::{
    auth, gst, Customer, Invoice, InvoiceDetail, InvoiceFilter, InvoiceLine, InvoiceSummary, Item,
    NewCustomer, NewInvoice, NewInvoiceLine, NewUser, Role, User,
};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::{AppState, Session, NODE_NAME};

// ------------------------------------------------------------------- sign-in

/// What the sign-in screen needs before anyone has signed in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStatus {
    pub node: String,
    pub session: Option<Session>,
    /// True when this installation has no accounts at all, so the shell asks for one to
    /// be created instead of asking to sign in to an account that does not exist.
    pub needs_setup: bool,
}

/// Whether anyone is signed in, and whether this copy has been set up at all. One of the
/// three commands — with `create_first_user` and `login` — that work without a session;
/// the shell asks it on startup to decide what to show.
#[tauri::command]
pub fn auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    let count = state.db().count_users().map_err(|e| e.to_string())?;
    Ok(AuthStatus {
        node: NODE_NAME.to_string(),
        session: state.session(),
        needs_setup: count == 0,
    })
}

/// Signs in. One message for a bad username and a bad password alike, so the screen
/// cannot be used to find out which accounts exist.
#[tauri::command]
pub fn login(
    username: String,
    password: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    let found = state.db().verify_login(&username, &password).map_err(|e| e.to_string())?;

    match found {
        Some(user) => {
            let session = Session { token: auth::new_session_token(), user };
            state.begin_session(session.clone());
            Ok(session)
        }
        None => Err("Incorrect username or password.".to_string()),
    }
}

/// Signs out, clearing the in-memory session. Every other command starts failing again.
#[tauri::command]
pub fn logout(state: State<'_, AppState>) {
    state.end_session();
}

/// Creates the very first account and signs it in.
///
/// Unauthenticated by necessity — there is nobody to authenticate against yet — so it is
/// guarded by the only thing that makes it safe: it refuses outright once an account
/// exists. The check and the insert share the database lock, so a second caller racing
/// the first finds the table populated and is turned away. The role is not a parameter:
/// whoever sets the machine up owns it.
#[tauri::command]
pub fn create_first_user(
    display_name: String,
    username: String,
    password: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    let user = {
        let mut db = state.db();
        if db.count_users().map_err(|e| e.to_string())? > 0 {
            return Err("This copy of RealInvoice has already been set up.".to_string());
        }
        let new = NewUser {
            username: username.trim().to_string(),
            display_name: display_name.trim().to_string(),
            role: Role::Owner,
        };
        db.create_user(&new, &password).map_err(|e| e.to_string())?
    };

    // Straight in. Making someone re-type what they just typed proves nothing.
    let session = Session { token: auth::new_session_token(), user };
    state.begin_session(session.clone());
    Ok(session)
}

/// Every account on this machine. Owner-only: who else can sign in is not a cashier's
/// business.
#[tauri::command]
pub fn list_users(state: State<'_, AppState>) -> Result<Vec<User>, String> {
    require_owner(&state)?;
    state.db().list_users().map_err(|e| e.to_string())
}

/// Adds an account. Owner-only, and the caller picks the role.
#[tauri::command]
pub fn create_user(
    display_name: String,
    username: String,
    password: String,
    role: String,
    state: State<'_, AppState>,
) -> Result<User, String> {
    require_owner(&state)?;
    let role = Role::parse(&role).ok_or_else(|| "Unknown role.".to_string())?;
    let new = NewUser {
        username: username.trim().to_string(),
        display_name: display_name.trim().to_string(),
        role,
    };

    let mut db = state.db();
    // Said in the words of somebody running a shop, rather than core's field-level
    // wording. The UNIQUE constraint inside `create_user` is still what guarantees it —
    // this only decides what the screen says in the case that actually happens.
    if db.find_user(&new.username).map_err(|e| e.to_string())?.is_some() {
        return Err("That username is already taken.".to_string());
    }
    db.create_user(&new, &password).map_err(|e| e.to_string())
}

/// The gate every other command goes through.
///
/// Hiding the shell in the frontend is presentation; this is what actually stops an
/// unauthenticated caller reading customers or writing an invoice.
fn require_session(state: &State<'_, AppState>) -> Result<Session, String> {
    session_of(state.session())
}

/// The gate on the commands that manage accounts.
///
/// Hiding the Users screen from cashiers is presentation; this is what stops one calling
/// `create_user` directly and promoting itself.
fn require_owner(state: &State<'_, AppState>) -> Result<Session, String> {
    owner_of(state.session())
}

/// [`require_session`] without the Tauri handle, so it can be tested.
pub(crate) fn session_of(session: Option<Session>) -> Result<Session, String> {
    session.ok_or_else(|| "Not signed in.".to_string())
}

/// [`require_owner`] without the Tauri handle, so it can be tested. Not being signed in
/// and being signed in as a cashier are both refusals — there is no third answer.
pub(crate) fn owner_of(session: Option<Session>) -> Result<Session, String> {
    let session = session_of(session)?;
    match session.user.role {
        Role::Owner => Ok(session),
        _ => Err("Only an owner can manage accounts.".to_string()),
    }
}

/// Test hooks for the two gates, which the commands themselves need a running app to
/// reach.
#[doc(hidden)]
pub fn require_session_for_test(session: Option<Session>) -> Result<Session, String> {
    session_of(session)
}

#[doc(hidden)]
pub fn require_owner_for_test(session: Option<Session>) -> Result<Session, String> {
    owner_of(session)
}

/// What the About pane shows. Every field comes from the running binary, so bumping the
/// version in one place is reflected here without a second edit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub identifier: String,
    pub node: String,
    pub build_date: String,
    pub db_path: String,
}

/// App name, version and build date, for the About pane.
#[tauri::command]
pub fn app_info(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<AppInfo, String> {
    require_session(&state)?;
    let package = app.package_info();
    Ok(AppInfo {
        name: package.name.clone(),
        // Tauri's own idea of the app version — the one that ends up in the bundle.
        version: package.version.to_string(),
        identifier: app.config().identifier.clone(),
        node: NODE_NAME.to_string(),
        build_date: env!("REALINVOICE_BUILD_DATE").to_string(),
        db_path: state.db_path().display().to_string(),
    })
}

/// The themes the console ships with.
const THEMES: [&str; 2] = ["light", "dark"];

/// Key under which the chosen theme is stored.
const THEME_KEY: &str = "ui.theme";

/// The saved theme, or `None` to follow the operating system.
///
/// Reachable without a session: the login screen is themed too, and a display preference
/// exposes nothing.
#[tauri::command]
pub fn get_theme(state: State<'_, AppState>) -> Result<Option<String>, String> {
    state.db().get_setting(THEME_KEY).map_err(|e| e.to_string())
}

/// Saves the chosen theme so it survives a restart.
#[tauri::command]
pub fn set_theme(theme: String, state: State<'_, AppState>) -> Result<(), String> {
    if !THEMES.contains(&theme.as_str()) {
        return Err(format!("unknown theme: {theme}"));
    }
    state.db().set_setting(THEME_KEY, &theme).map_err(|e| e.to_string())
}

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
    /// Who is signed in — shown in the title bar beside the node name.
    pub user: Option<realinvoice_core::User>,
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
            // Never sent by the frontend — `create_invoice` fills it from the session.
            created_by_user_id: None,
            lines: payload.lines.into_iter().map(Into::into).collect(),
        }
    }
}

/// Title-bar state for the shell.
#[tauri::command]
pub fn node_status(state: State<'_, AppState>) -> Result<NodeStatus, String> {
    let session = require_session(&state)?;
    Ok(NodeStatus {
        node: NODE_NAME.to_string(),
        // Stub: real connectivity arrives with the sync stage.
        connected: true,
        db_path: state.db_path().display().to_string(),
        user: Some(session.user),
    })
}

/// Finds a customer by exact mobile number. `None` when nobody matches.
#[tauri::command]
pub fn search_customer(
    mobile: String,
    state: State<'_, AppState>,
) -> Result<Option<Customer>, String> {
    require_session(&state)?;
    state.db().search_customer(&mobile).map_err(|e| e.to_string())
}

/// Substring search over item code and description.
#[tauri::command]
pub fn search_item(query: String, state: State<'_, AppState>) -> Result<Vec<Item>, String> {
    require_session(&state)?;
    state.db().search_item(&query).map_err(|e| e.to_string())
}

/// Saves the transaction. Core does the work in one transaction: allocate the next
/// number in the financial year, insert the invoice, insert every line, and queue the
/// `sync_queue` rows. Nothing is written if any part of it fails, so a failed save leaves
/// the screen editable with nothing lost.
///
/// `expected` is what the summary panel was showing. When the rows carry explicit prices
/// — which the billing card always sends — the same figures are recomputed here first and
/// the save is refused on a mismatch, so an invoice can never be stored under numbers the
/// operator did not see.
#[tauri::command]
pub fn create_invoice(
    payload: NewInvoicePayload,
    expected: Option<ExpectedTotals>,
    state: State<'_, AppState>,
) -> Result<SavedInvoice, String> {
    let session = require_session(&state)?;

    let mut new_invoice = NewInvoice::from(payload);
    // Attribution is derived, never supplied: the cashier does not pick their own name,
    // and a caller cannot bill as somebody else.
    new_invoice.created_by_user_id = Some(session.user.id);

    // One guard for the whole operation: the pre-check, the save and the read-back all
    // see the same database state.
    let mut db = state.db();

    let customer = db
        .get_customer(new_invoice.customer_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("customer {} no longer exists", new_invoice.customer_id))?;

    if let Some(expected) = expected {
        check_expected_totals(&expected, &new_invoice, &customer.place_of_supply, db.home_state())?;
    }

    let invoice = db.create_invoice(&new_invoice).map_err(|e| e.to_string())?;
    let lines = db.invoice_lines(invoice.id).map_err(|e| e.to_string())?;
    let queued_sync_rows = db.pending_sync_rows().map_err(|e| e.to_string())?.len();

    Ok(SavedInvoice { invoice, lines, customer, queued_sync_rows })
}

/// Re-prices the rows and compares against what the panel displayed.
///
/// Only meaningful when every row carries its own rate and tax rate; a row that defers to
/// the item master is priced inside core during the save, and re-deriving that here would
/// mean a second copy of core's pricing rules. Those rows skip the check rather than get
/// a guess.
pub(crate) fn check_expected_totals(
    expected: &ExpectedTotals,
    new_invoice: &NewInvoice,
    place_of_supply: &str,
    home_state: &str,
) -> Result<(), String> {
    let mut priced = Vec::with_capacity(new_invoice.lines.len());
    for line in &new_invoice.lines {
        match (line.rate, line.tax_rate) {
            (Some(rate), Some(tax_rate)) => {
                priced.push(gst::TaxableLine { qty: line.qty, rate, tax_rate })
            }
            _ => return Ok(()),
        }
    }

    let actual = gst::compute_totals(&priced, home_state, place_of_supply);
    let differs = [
        ("subtotal", expected.subtotal, actual.subtotal),
        ("CGST", expected.cgst, actual.cgst),
        ("SGST", expected.sgst, actual.sgst),
        ("IGST", expected.igst, actual.igst),
        ("grand total", expected.grand_total, actual.grand_total),
    ]
    .into_iter()
    .find(|(_, shown, computed)| (shown - computed).abs() > MONEY_EPSILON);

    match differs {
        Some((field, shown, computed)) => Err(format!(
            "refusing to save: {field} on screen is {shown:.2} but prices to {computed:.2}"
        )),
        None => Ok(()),
    }
}

/// Appends a line to an existing invoice and re-totals it.
#[tauri::command]
pub fn add_line_item(
    invoice_id: i64,
    line: NewLinePayload,
    state: State<'_, AppState>,
) -> Result<InvoiceLine, String> {
    require_session(&state)?;
    state.db().add_line_item(invoice_id, &NewInvoiceLine::from(line)).map_err(|e| e.to_string())
}

/// The history query: date range plus a substring of customer name or invoice number.
/// An empty filter lists everything, newest first.
#[tauri::command]
pub fn list_invoices(
    filter: Option<InvoiceFilter>,
    state: State<'_, AppState>,
) -> Result<Vec<InvoiceSummary>, String> {
    require_session(&state)?;
    state.db().list_invoices(&filter.unwrap_or_default()).map_err(|e| e.to_string())
}

/// One saved invoice with its buyer and lines, for the read-only detail view and for
/// reprinting. There is deliberately no counterpart that changes a stored invoice:
/// invoices are append-only once saved.
#[tauri::command]
pub fn invoice_detail(
    invoice_id: i64,
    state: State<'_, AppState>,
) -> Result<Option<InvoiceDetail>, String> {
    require_session(&state)?;
    state.db().get_invoice_detail(invoice_id).map_err(|e| e.to_string())
}

/// Today's invoices, newest first. An empty list on a quiet morning — not an error.
#[tauri::command]
pub fn list_todays_invoices(state: State<'_, AppState>) -> Result<Vec<Invoice>, String> {
    require_session(&state)?;
    state.db().list_todays_invoices().map_err(|e| e.to_string())
}

/// The totals the operator was shown when they hit Print & Lock. Sent so the save can
/// refuse if what core prices differs from what was on screen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedTotals {
    pub subtotal: f64,
    pub cgst: f64,
    pub sgst: f64,
    pub igst: f64,
    pub grand_total: f64,
}

/// Everything the locked screen needs after a successful save: the invoice with its
/// freshly allocated number, the lines as they were stored, the buyer, and how many rows
/// are now waiting in `sync_queue` for a later stage to send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedInvoice {
    pub invoice: Invoice,
    pub lines: Vec<InvoiceLine>,
    pub customer: Customer,
    pub queued_sync_rows: usize,
}

/// Half a paisa — closer than two roundings of the same figure can differ.
const MONEY_EPSILON: f64 = 0.005;

/// Registers a customer the counter could not resolve. Errors if the mobile number is
/// already on file.
#[tauri::command]
pub fn create_customer(
    payload: NewCustomerPayload,
    state: State<'_, AppState>,
) -> Result<Customer, String> {
    require_session(&state)?;
    state.db().create_customer(&NewCustomer::from(payload)).map_err(|e| e.to_string())
}

/// Prices the rows currently on screen against a place of supply, without touching the
/// database. Pure and cheap, so the summary panel can re-quote on every edit; it exists
/// so the live totals and the saved invoice come from one implementation.
#[tauri::command]
pub fn quote_invoice(
    place_of_supply: String,
    lines: Vec<QuoteLinePayload>,
    state: State<'_, AppState>,
) -> Result<InvoiceQuote, String> {
    require_session(&state)?;
    Ok(quote(&place_of_supply, &lines))
}

/// The pricing behind [`quote_invoice`], with no session or app state involved.
pub fn quote(place_of_supply: &str, lines: &[QuoteLinePayload]) -> InvoiceQuote {
    let home_state = realinvoice_core::DEFAULT_HOME_STATE;
    let taxable: Vec<gst::TaxableLine> = lines
        .iter()
        .map(|l| gst::TaxableLine { qty: l.qty, rate: l.rate, tax_rate: l.tax_rate })
        .collect();

    let totals = gst::compute_totals(&taxable, home_state, place_of_supply);
    InvoiceQuote {
        home_state: home_state.to_string(),
        intra_state: gst::is_intra_state(home_state, place_of_supply),
        line_totals: taxable.iter().map(|l| l.line_total()).collect(),
        subtotal: totals.subtotal,
        cgst: totals.cgst,
        sgst: totals.sgst,
        igst: totals.igst,
        grand_total: totals.grand_total,
    }
}

/// Test hook for [`check_expected_totals`], which is otherwise an implementation detail
/// of `create_invoice`. The command itself needs a running app to call.
#[doc(hidden)]
pub fn check_expected_totals_for_test(
    expected: &ExpectedTotals,
    new_invoice: &NewInvoice,
    place_of_supply: &str,
    home_state: &str,
) -> Result<(), String> {
    check_expected_totals(expected, new_invoice, place_of_supply, home_state)
}
