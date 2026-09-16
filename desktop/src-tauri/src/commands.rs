//! The `invoke()` surface.
//!
//! Every command here is a thin wrapper: lock the shared [`crate::state::AppState`], call
//! one core function, hand the result back. No billing logic lives in this crate — GST,
//! numbering and the `sync_queue` writes all belong to `realinvoice-core`, so the Phoenix
//! and Ratatui runtimes get identical behaviour for free.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use realinvoice_core::{
    auth, gst, sync, CreditNote, CreditNoteDetail, CreditableLine, Customer, DailyTotal, DateRange,
    DemoDataCleared, Invoice, InvoiceDetail, InvoiceFilter, InvoiceLine, InvoiceNet,
    InvoiceSummary, Item, ItemFilter, ItemPrice, ItemPriceRow, Lockout, LoginOutcome,
    NewCreditNote, NewCreditNoteLine, NewCustomer, NewInvoice, NewInvoiceLine, NewItem, NewUser,
    PaymentMix, PriceList, PricedItem, ResolvedRate, Role, SalesSummary, SyncStatus, TopItem, User,
    LOGIN_WINDOW_MINUTES, MAX_FAILED_LOGINS,
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

/// Why a sign-in did not work, in a shape the screen can act on.
///
/// A plain string would be enough to print but not enough to count down with, and the
/// locked-out case has to look different from a wrong password — telling somebody their
/// password is wrong when it is right, because the account is shut out, is the failure
/// this whole feature exists to avoid.
#[derive(Debug, Clone, Serialize)]
pub struct LoginError {
    /// `"invalid"`, `"locked_out"` or `"error"`.
    pub kind: String,
    /// What to show. Already in the words a cashier should read.
    pub message: String,
    /// Seconds until another attempt is allowed. Only on `locked_out`.
    pub retry_after_seconds: Option<i64>,
    /// Attempts left before a lockout. Only on `invalid`, and only once it is worth
    /// warning about — being told "4 left" on a first typo is noise.
    pub attempts_remaining: Option<i64>,
}

impl LoginError {
    /// Wrong username or wrong password: one message for both, so the screen cannot be
    /// used to find out which accounts exist.
    fn invalid(failures: i64) -> Self {
        let remaining = (MAX_FAILED_LOGINS - failures).max(0);
        // Warn only when it is nearly spent. A warning on every typo trains people to
        // ignore it, and the one that matters is the last.
        let attempts_remaining = (remaining <= 2 && remaining > 0).then_some(remaining);

        // The window is quoted from the constant rather than written out, so changing the
        // lockout length cannot leave the screen promising the old one.
        let window = describe_window();
        let message = match attempts_remaining {
            Some(1) => format!(
                "Incorrect username or password. 1 attempt left before this account is \
                 locked for {window}."
            ),
            Some(left) => format!(
                "Incorrect username or password. {left} attempts left before this \
                 account is locked for {window}."
            ),
            None => "Incorrect username or password.".to_string(),
        };

        Self { kind: "invalid".to_string(), message, retry_after_seconds: None, attempts_remaining }
    }

    fn locked_out(lockout: &Lockout) -> Self {
        Self {
            kind: "locked_out".to_string(),
            message: format!(
                "Too many failed attempts — try again in {}.",
                describe_wait(lockout.retry_after_seconds)
            ),
            retry_after_seconds: Some(lockout.retry_after_seconds),
            attempts_remaining: Some(0),
        }
    }
}

impl From<realinvoice_core::CoreError> for LoginError {
    fn from(err: realinvoice_core::CoreError) -> Self {
        Self {
            kind: "error".to_string(),
            message: err.to_string(),
            retry_after_seconds: None,
            attempts_remaining: None,
        }
    }
}

/// How long a lockout lasts, in words, straight from core's constant.
pub fn describe_window() -> String {
    match LOGIN_WINDOW_MINUTES {
        1 => "a minute".to_string(),
        n => format!("{n} minutes"),
    }
}

/// Seconds as something a person would say. Rounded up, because being told "1 minute" and
/// still being refused at 55 seconds reads as the app lying.
pub fn describe_wait(seconds: i64) -> String {
    if seconds <= 60 {
        return "less than a minute".to_string();
    }
    // Ceiling, so 61 seconds is quoted as 2 minutes. Over-stating the wait by a few
    // seconds costs nothing; under-stating it is the app telling somebody to come back
    // at a moment when it will still refuse them.
    let minutes = (seconds + 59) / 60;
    format!("{minutes} minutes")
}

/// Signs in, subject to core's rate limit. One message for a bad username and a bad
/// password alike, so the screen cannot be used to find out which accounts exist.
#[tauri::command]
pub fn login(
    username: String,
    password: String,
    state: State<'_, AppState>,
) -> Result<Session, LoginError> {
    let outcome = state.db().attempt_login(&username, &password).map_err(LoginError::from)?;

    match outcome {
        LoginOutcome::Ok(user) => {
            let session = Session { token: auth::new_session_token(), user };
            state.begin_session(session.clone());
            Ok(session)
        }
        LoginOutcome::Invalid => {
            Err(LoginError::invalid(state.db().recent_failed_logins(&username).unwrap_or(0)))
        }
        LoginOutcome::LockedOut(lockout) => Err(LoginError::locked_out(&lockout)),
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
    /// Absent or null means the default list, which is what the form sends when nobody
    /// picks one.
    #[serde(default)]
    pub price_list_id: Option<i64>,
}

impl From<NewCustomerPayload> for NewCustomer {
    fn from(payload: NewCustomerPayload) -> Self {
        NewCustomer {
            name: payload.name,
            gstin: payload.gstin,
            place_of_supply: payload.place_of_supply,
            mobile: payload.mobile,
            price_list_id: payload.price_list_id,
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
        // Real, now: connected means the push worker is getting through. The badge reads
        // `sync_status` for the detail; this stays for the rest of the title bar.
        connected: state.sync().map(|s| healthy(&s.status())).unwrap_or(false),
        db_path: state.db_path().display().to_string(),
        user: Some(session.user),
    })
}

// -------------------------------------------------------------------- sync

/// Whether the badge should read connected.
///
/// True when the last batch the back office accepted landed within twice the poll
/// interval — one missed poll is a slow network, two is a problem worth showing. An
/// unconfigured node is not connected and not failing; it is waiting to be pointed at a
/// back office, which the screen says in those words rather than in red.
pub fn healthy(status: &SyncStatus) -> bool {
    if !status.configured || !status.token_set || status.token_rejected {
        return false;
    }
    if status.consecutive_failures > 0 {
        return false;
    }

    match status.last_success.as_deref().and_then(parse_stamp) {
        Some(at) => {
            let age = Local::now().signed_duration_since(at).num_seconds();
            age >= 0 && age <= (status.poll_seconds as i64) * 2
        }
        None => false,
    }
}

/// `YYYY-MM-DD HH:MM:SS` in local time, as core writes it.
fn parse_stamp(stamp: &str) -> Option<DateTime<Local>> {
    NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M:%S")
        .ok()
        .and_then(|naive| Local.from_local_datetime(&naive).single())
}

/// What the badge and the Sync screen show.
#[derive(Debug, Clone, Serialize)]
pub struct SyncView {
    #[serde(flatten)]
    pub status: SyncStatus,
    /// The badge's verdict, decided here rather than in JavaScript so every runtime that
    /// shows this agrees on what "connected" means.
    pub healthy: bool,
}

/// The push worker's current state. Reachable by any signed-in user: a cashier who can
/// see the badge should be able to see why it is amber.
#[tauri::command]
pub fn sync_status(state: State<'_, AppState>) -> Result<SyncView, String> {
    require_session(&state)?;

    match state.sync() {
        Some(handle) => {
            let mut status = handle.status();
            // The worker refreshes this on each poll; reading it here keeps the count
            // honest between polls, which is what a cashier watching it expects.
            status.pending = state.db().pending_sync_count().map_err(|e| e.to_string())?;
            Ok(SyncView { healthy: healthy(&status), status })
        }
        None => Err("The sync worker is not running.".to_string()),
    }
}

/// Asks the worker to poll now instead of waiting out its timer.
///
/// Returns as soon as the nudge is delivered. It deliberately does not wait for the push
/// to finish: the button must not hang the screen on a network round trip, and the status
/// the UI polls will show the result a moment later.
#[tauri::command]
pub fn sync_now(state: State<'_, AppState>) -> Result<(), String> {
    require_session(&state)?;
    state.sync().ok_or_else(|| "The sync worker is not running.".to_string())?.sync_now();
    Ok(())
}

/// Stores the token the back office issued for this node. Owner-only.
///
/// The token is written to core's `settings` table — this node's own SQLite file — and
/// nowhere else. It is never returned to the frontend afterwards: `sync_status` carries
/// only whether one is set and its last four characters, because anything that reaches
/// JavaScript can be read out of the page.
///
/// Saving lifts a rejection. Somebody pasting a credential is saying "try this one", and
/// a worker that stayed disabled until the app restarted would be the wrong answer to
/// exactly the situation this command exists to fix.
#[tauri::command]
pub fn set_sync_token(token: String, state: State<'_, AppState>) -> Result<SyncView, String> {
    require_owner(&state)?;
    let token = token.trim().to_string();

    state.db().set_setting(sync::TOKEN_KEY, &token).map_err(|e| e.to_string())?;

    if let Some(handle) = state.sync() {
        handle.token_changed(&token);
        if !token.is_empty() {
            // Try it straight away, so somebody who has just pasted a token finds out now
            // whether it works rather than in ten seconds.
            handle.sync_now();
        }
    }
    sync_status(state)
}

/// Points this node at a back office. Owner-only: where a shop's invoices are sent is not
/// a cashier's decision.
///
/// An empty value clears it, which stops the worker attempting anything rather than
/// leaving it retrying against a URL nobody meant.
#[tauri::command]
pub fn set_sync_endpoint(endpoint: String, state: State<'_, AppState>) -> Result<String, String> {
    require_owner(&state)?;
    let endpoint = endpoint.trim().to_string();

    // Empty is legitimate — it clears the address and stops the worker attempting
    // anything. Anything else has to be a URL the HTTP client can actually post to.
    let usable =
        endpoint.is_empty() || endpoint.starts_with("http://") || endpoint.starts_with("https://");
    if !usable {
        return Err("The address must start with http:// or https://".to_string());
    }

    state.db().set_setting(sync::ENDPOINT_KEY, &endpoint).map_err(|e| e.to_string())?;
    if let Some(handle) = state.sync() {
        handle.set_endpoint(&endpoint);
        // Try it straight away, so somebody who has just pasted an address finds out now
        // whether it works rather than in ten seconds.
        handle.sync_now();
    }
    Ok(endpoint)
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
pub fn search_item(
    query: String,
    price_list_id: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<PricedItem>, String> {
    require_session(&state)?;
    let db = state.db();
    // No list named means nobody is attached yet, so the picker shows what a walk-in
    // would pay. Attaching a customer re-prices the bill rather than leaving those
    // numbers standing.
    let list_id = match price_list_id {
        Some(id) => id,
        None => db.default_price_list().map_err(|e| e.to_string())?.id,
    };
    db.search_item_priced(&query, list_id).map_err(|e| e.to_string())
}

// ------------------------------------------------------------- price lists

/// Every price list, default first. Open to any signed-in user: a cashier cannot change
/// them, but the billing screen has to be able to say which one is being applied.
#[tauri::command]
pub fn price_lists(state: State<'_, AppState>) -> Result<Vec<PriceList>, String> {
    require_session(&state)?;
    state.db().price_lists().map_err(|e| e.to_string())
}

/// Which list a customer is billed from — theirs, or the default.
#[tauri::command]
pub fn price_list_for_customer(
    customer_id: i64,
    state: State<'_, AppState>,
) -> Result<PriceList, String> {
    require_session(&state)?;
    state.db().price_list_for_customer(customer_id).map_err(|e| e.to_string())
}

/// The list to price against before anyone is attached.
#[tauri::command]
pub fn default_price_list(state: State<'_, AppState>) -> Result<PriceList, String> {
    require_session(&state)?;
    state.db().default_price_list().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_price_list(name: String, state: State<'_, AppState>) -> Result<PriceList, String> {
    require_owner(&state)?;
    state.db().create_price_list(&name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_price_list(
    id: i64,
    name: String,
    state: State<'_, AppState>,
) -> Result<PriceList, String> {
    require_owner(&state)?;
    state.db().rename_price_list(id, &name).map_err(|e| e.to_string())
}

/// Moves the default flag. Un-defaulting the previous list is core's job, in the same
/// transaction, so the two can never both be set or both be clear.
#[tauri::command]
pub fn set_default_price_list(id: i64, state: State<'_, AppState>) -> Result<PriceList, String> {
    require_owner(&state)?;
    state.db().set_default_price_list(id).map_err(|e| e.to_string())
}

/// Puts a customer on a list, or back on the default with `null`.
///
/// Owner-only. What a buyer pays is a commercial decision, not a counter one: a cashier
/// who could move a customer to Wholesale mid-sale could discount any bill at will.
#[tauri::command]
pub fn set_customer_price_list(
    customer_id: i64,
    price_list_id: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Customer, String> {
    require_owner(&state)?;
    state.db().set_customer_price_list(customer_id, price_list_id).map_err(|e| e.to_string())
}

/// Every list with this item's rate on it, for the item edit form.
#[tauri::command]
pub fn item_prices(item_id: i64, state: State<'_, AppState>) -> Result<Vec<ItemPriceRow>, String> {
    require_session(&state)?;
    state.db().item_prices(item_id).map_err(|e| e.to_string())
}

/// Replaces an item's price table with exactly what the form holds.
///
/// A list left blank on the form is simply absent here, and core removes its row — which
/// is what makes an empty box mean "falls back to the base rate" rather than "unchanged".
#[tauri::command]
pub fn set_item_prices(
    item_id: i64,
    prices: Vec<ItemPrice>,
    state: State<'_, AppState>,
) -> Result<Vec<ItemPriceRow>, String> {
    require_owner(&state)?;
    let mut db = state.db();
    db.set_item_prices(item_id, &prices).map_err(|e| e.to_string())?;
    db.item_prices(item_id).map_err(|e| e.to_string())
}

/// What these items cost on this list, for repricing a bill in one call.
///
/// The billing screen calls this when a customer is attached after items are already on
/// the bill: the rates it assumed were the default list's, and this is what they should
/// have been.
#[tauri::command]
pub fn resolve_rates(
    item_ids: Vec<i64>,
    price_list_id: i64,
    state: State<'_, AppState>,
) -> Result<Vec<ResolvedRate>, String> {
    require_session(&state)?;
    state.db().resolve_rates(&item_ids, price_list_id).map_err(|e| e.to_string())
}

// ------------------------------------------------------------- credit notes

/// What is still creditable on an invoice, for drawing the form.
///
/// Owner-only, like issuing one: a cashier who cannot reverse an invoice has no reason to
/// be shown the screen for it.
#[tauri::command]
pub fn creditable_lines(
    invoice_id: i64,
    state: State<'_, AppState>,
) -> Result<Vec<CreditableLine>, String> {
    require_owner(&state)?;
    state.db().creditable_lines(invoice_id).map_err(|e| e.to_string())
}

/// What the form sends.
///
/// Deliberately carries no `created_by_user_id`. Attribution comes from the session, the
/// same rule invoices follow — a caller must not be able to record a reversal as somebody
/// else's decision.
#[derive(Debug, Clone, Deserialize)]
pub struct NewCreditNotePayload {
    pub original_invoice_id: i64,
    pub reason: String,
    pub lines: Vec<CreditLinePayload>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreditLinePayload {
    pub invoice_line_id: i64,
    pub qty: f64,
}

/// Issues a credit note. **Owner-only.**
///
/// Reversing an invoice is not a counter decision: a cashier who could issue one could
/// void their own sales, which is the shape of most till fraud. The Users screen gates on
/// the same role for the same reason, and this is enforced here rather than only by
/// hiding the button.
#[tauri::command]
pub fn create_credit_note(
    note: NewCreditNotePayload,
    state: State<'_, AppState>,
) -> Result<CreditNote, String> {
    let session = require_owner(&state)?;

    state
        .db()
        .create_credit_note(&NewCreditNote {
            original_invoice_id: note.original_invoice_id,
            reason: note.reason,
            date: None,
            // From the session, never from the payload.
            created_by_user_id: Some(session.user.id),
            lines: note
                .lines
                .into_iter()
                .map(|l| NewCreditNoteLine { invoice_line_id: l.invoice_line_id, qty: l.qty })
                .collect(),
        })
        .map_err(|e| e.to_string())
}

/// An invoice's credit notes and what they net it down to.
///
/// Readable by anyone signed in, unlike issuing one: a cashier looking at an invoice
/// should see that it was partly returned, or the figure on their screen is wrong.
#[derive(Debug, Clone, Serialize)]
pub struct CreditHistory {
    pub notes: Vec<CreditNoteDetail>,
    pub net: InvoiceNet,
}

#[tauri::command]
pub fn credit_history(
    invoice_id: i64,
    state: State<'_, AppState>,
) -> Result<CreditHistory, String> {
    require_session(&state)?;
    let db = state.db();
    Ok(CreditHistory {
        notes: db.credit_notes_for_invoice(invoice_id).map_err(|e| e.to_string())?,
        net: db.invoice_net(invoice_id).map_err(|e| e.to_string())?,
    })
}

/// The catalogue, for the Inventory pane. Wider than [`search_item`], which exists to
/// feed the billing picker and stops at 50.
#[tauri::command]
pub fn list_items(filter: ItemFilter, state: State<'_, AppState>) -> Result<Vec<Item>, String> {
    require_session(&state)?;
    state.db().list_items(&filter).map_err(|e| e.to_string())
}

/// How many items exist, whatever the list is filtered to.
#[tauri::command]
pub fn count_items(state: State<'_, AppState>) -> Result<i64, String> {
    require_session(&state)?;
    state.db().count_items().map_err(|e| e.to_string())
}

/// What the Inventory form sends. A separate type from `NewItem` so the numbers can
/// arrive as whatever the input produced and be validated here, rather than failing to
/// deserialize and reaching the screen as a parser error.
#[derive(Debug, Clone, Deserialize)]
pub struct NewItemPayload {
    pub item_code: String,
    pub description: String,
    pub rate: f64,
    pub tax_rate: f64,
    pub uom: String,
}

/// Adds an item, or updates the one that already has that code.
///
/// Editing is the same call as adding because core keys items on `item_code`: a shop
/// changing a price is doing the same thing as a shop adding the line for the first time.
/// Unlike accounts, the catalogue is shared business data, so this is queued for sync.
#[tauri::command]
pub fn save_item(item: NewItemPayload, state: State<'_, AppState>) -> Result<Item, String> {
    require_session(&state)?;
    let new = check_item(&item)?;
    state.db().upsert_item(&new).map_err(|e| e.to_string())
}

/// Validates and normalises an item from the Inventory form.
///
/// Caught here rather than left to SQLite: a negative price or a tax rate of 900% is a
/// typo, and the person who made it is standing at the till. `rate` is rounded to paise
/// by the same function that rounds every other amount in this product, so a price cannot
/// enter the catalogue carrying a fraction of a paisa that later shows up in a total.
fn check_item(item: &NewItemPayload) -> Result<NewItem, String> {
    if !item.rate.is_finite() || item.rate < 0.0 {
        return Err("Rate must be a number, and cannot be negative.".to_string());
    }
    if !item.tax_rate.is_finite() || !(0.0..=100.0).contains(&item.tax_rate) {
        return Err("Tax % must be between 0 and 100.".to_string());
    }

    let uom = item.uom.trim();
    Ok(NewItem {
        item_code: item.item_code.trim().to_string(),
        description: item.description.trim().to_string(),
        rate: gst::round_money(item.rate),
        tax_rate: item.tax_rate,
        // A unit is never blank on a bill; NOS is what a counter means by "each".
        uom: if uom.is_empty() { "NOS".to_string() } else { uom.to_string() },
        // Saved from the Inventory screen, so it is a catalogue item by definition.
        custom: false,
    })
}

/// Test hook for [`check_item`], which the command itself needs a running app to reach.
#[doc(hidden)]
pub fn check_item_for_test(item: &NewItemPayload) -> Result<NewItem, String> {
    check_item(item)
}

/// Adds a one-off item for the bill in hand, outside the catalogue.
///
/// Any signed-in user, unlike `save_item`: billing something not on the price list is an
/// ordinary counter action, and a cashier who had to fetch the owner to sell a delivery
/// charge would go on billing it under the wrong line instead. It does not touch the
/// catalogue, which is what makes it safe to leave open.
#[tauri::command]
pub fn create_custom_item(
    description: String,
    rate: f64,
    tax_rate: f64,
    uom: String,
    state: State<'_, AppState>,
) -> Result<Item, String> {
    require_session(&state)?;
    state.db().create_custom_item(&description, rate, tax_rate, &uom).map_err(|e| e.to_string())
}

/// Removes the demo catalogue a fresh install ships with. **Owner-only.**
///
/// Destructive and one-way, so it sits behind the same role as the Users screen. Only
/// untouched sample rows go: anything an invoice or credit note still refers to stays,
/// and the result says how many were kept so the screen can report that honestly rather
/// than claiming a clean sweep.
#[tauri::command]
pub fn clear_demo_data(state: State<'_, AppState>) -> Result<DemoDataCleared, String> {
    require_owner(&state)?;
    state.db().clear_demo_data().map_err(|e| e.to_string())
}

/// Everything the Analytics pane draws, in one round trip.
///
/// Bundled deliberately: four separate commands would be four locks of the same
/// connection and four chances for the screen to show figures from four different
/// instants. These all come from one read of one database.
#[derive(Debug, Clone, Serialize)]
pub struct AnalyticsReport {
    pub summary: SalesSummary,
    pub daily: Vec<DailyTotal>,
    pub top_items: Vec<TopItem>,
    pub payments: Vec<PaymentMix>,
}

/// The figures behind the Analytics pane. Every one of them is added up by core; nothing
/// here computes money, and neither does the JavaScript that displays it.
#[tauri::command]
pub fn analytics(range: DateRange, state: State<'_, AppState>) -> Result<AnalyticsReport, String> {
    require_session(&state)?;
    let db = state.db();
    Ok(AnalyticsReport {
        summary: db.sales_summary(&range).map_err(|e| e.to_string())?,
        daily: db.daily_totals(&range).map_err(|e| e.to_string())?,
        top_items: db.top_items(&range, 8).map_err(|e| e.to_string())?,
        payments: db.payment_mix(&range).map_err(|e| e.to_string())?,
    })
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
        // Priced by core, through this customer's price list, and only then compared with
        // what the screen showed. Before price lists the frontend sent the rates and this
        // check could only confirm core agreed with itself; now it catches the case that
        // matters — a bill totalled at retail for a customer the database has on
        // wholesale.
        let priced = db.price_invoice_lines(&new_invoice).map_err(|e| e.to_string())?;
        check_expected_totals(&expected, &priced, &customer.place_of_supply, db.home_state())?;
    }

    let invoice = db.create_invoice(&new_invoice).map_err(|e| e.to_string())?;
    let lines = db.invoice_lines(invoice.id).map_err(|e| e.to_string())?;
    let queued_sync_rows = db.pending_sync_rows().map_err(|e| e.to_string())?.len();

    Ok(SavedInvoice { invoice, lines, customer, queued_sync_rows })
}

/// Compares what the panel displayed against lines core has already priced.
///
/// The rates are not re-derived here — they are handed in from
/// [`realinvoice_core::Db::price_invoice_lines`], so there is exactly one copy of the
/// pricing rules and this function's only job is the comparison. A disagreement means the
/// screen is stale, most often because the customer's price list changed under it, and
/// the save is refused rather than quietly billing one of the two numbers.
pub(crate) fn check_expected_totals(
    expected: &ExpectedTotals,
    priced: &[gst::TaxableLine],
    place_of_supply: &str,
    home_state: &str,
) -> Result<(), String> {
    let actual = gst::compute_totals(priced, home_state, place_of_supply);
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
    priced: &[gst::TaxableLine],
    place_of_supply: &str,
    home_state: &str,
) -> Result<(), String> {
    check_expected_totals(expected, priced, place_of_supply, home_state)
}
