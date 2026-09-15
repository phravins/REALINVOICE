//! The Office Console's command layer, exercised without a window.
//!
//! Tauri commands can't be called outside a running app, so these tests drive the state
//! and payload conversions that sit behind them — the parts this crate actually owns.
//! What they prove: the console opens its own database file, seeds itself, and a frontend
//! payload survives the trip into core unchanged.

use realinvoice_core::{
    seed, DateRange, InvoiceFilter, LoginOutcome, NewCreditNote, NewCreditNoteLine, NewCustomer,
    NewInvoice, NewInvoiceLine, NewUser, Role, User,
};
use realinvoice_desktop_lib::commands::{
    check_expected_totals_for_test as check_expected_totals, check_item_for_test as check_item,
    describe_wait, describe_window, quote, require_owner_for_test as require_owner,
    require_session_for_test as require_session, ExpectedTotals, NewCreditNotePayload,
    NewCustomerPayload, NewInvoicePayload, NewItemPayload, NewLinePayload, QuoteLinePayload,
};
use realinvoice_desktop_lib::state::{AppState, Session, DB_FILE_NAME};

fn console_state() -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(dir.path().join(DB_FILE_NAME)).expect("open console db");
    (dir, state)
}

#[test]
fn startup_creates_a_seeded_database_file() {
    let (dir, state) = console_state();
    let path = dir.path().join(DB_FILE_NAME);

    assert!(path.exists(), "db.sqlite should be created at {}", path.display());
    assert_eq!(state.db_path(), path);

    let resolved = state.db().search_customer("9840012345").unwrap();
    assert_eq!(resolved.unwrap().name, "Sri Balaji Traders");
    assert!(!state.db().search_item("").unwrap().is_empty());
}

#[test]
fn reopening_the_same_path_keeps_the_data_and_does_not_reseed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);

    let invoice_no = {
        let state = AppState::new(path.clone()).unwrap();
        let buyer = state.db().search_customer("9840012345").unwrap().unwrap();
        let item = state.db().search_item("TMT").unwrap().remove(0);
        let invoice = state
            .db()
            .create_invoice(&NewInvoice {
                customer_id: buyer.id,
                date: None,
                payment_type: "cash".into(),
                created_by_user_id: None,
                lines: vec![NewInvoiceLine {
                    item_id: item.id,
                    qty: 2.0,
                    rate: None,
                    tax_rate: None,
                }],
            })
            .unwrap();
        invoice.invoice_no
    };

    let state = AppState::new(path).unwrap();
    let today = state.db().list_todays_invoices().unwrap();
    assert_eq!(today.len(), 1, "seeding must not re-run and wipe or duplicate anything");
    assert_eq!(today[0].invoice_no, invoice_no);
    assert_eq!(state.db().search_item("").unwrap().len(), seed::demo_items().len());
}

#[test]
fn an_empty_database_lists_no_invoices_without_erroring() {
    let (_dir, state) = console_state();
    assert!(state.db().list_todays_invoices().unwrap().is_empty());
}

#[test]
fn a_frontend_payload_converts_into_a_core_invoice() {
    // Exactly what invoke("create_invoice", { payload }) sends over the bridge.
    let json = r#"{
        "customer_id": 7,
        "payment_type": "upi",
        "lines": [
            { "item_id": 2, "qty": 3 },
            { "item_id": 5, "qty": 1.5, "rate": 99.5, "tax_rate": 5 }
        ]
    }"#;

    let payload: NewInvoicePayload = serde_json::from_str(json).unwrap();
    let new_invoice = NewInvoice::from(payload);

    assert_eq!(new_invoice.customer_id, 7);
    assert_eq!(new_invoice.payment_type, "upi");
    // `date` is optional on the wire and means "today" downstream.
    assert_eq!(new_invoice.date, None);
    assert_eq!(new_invoice.lines.len(), 2);
    // Omitted rate/tax_rate stay None so core prices off the item master.
    assert_eq!(new_invoice.lines[0].rate, None);
    assert_eq!(new_invoice.lines[0].tax_rate, None);
    assert_eq!(new_invoice.lines[1].rate, Some(99.5));
    assert_eq!(new_invoice.lines[1].tax_rate, Some(5.0));
}

#[test]
fn a_payload_posted_through_the_console_produces_a_real_invoice() {
    let (_dir, state) = console_state();
    let buyer = state.db().search_customer("9840012345").unwrap().unwrap();
    let cement = state.db().search_item("CEM-OPC-53").unwrap().remove(0);

    let payload = NewInvoicePayload {
        customer_id: buyer.id,
        date: None,
        payment_type: "cash".into(),
        lines: vec![NewLinePayload { item_id: cement.id, qty: 10.0, rate: None, tax_rate: None }],
    };
    let invoice = state.db().create_invoice(&NewInvoice::from(payload)).unwrap();

    // 10 x 410 = 4,100 @ 28% intra-state TN -> 574 CGST + 574 SGST.
    assert_eq!(invoice.subtotal, 4_100.00);
    assert_eq!(invoice.cgst, 574.00);
    assert_eq!(invoice.grand_total, 5_248.00);

    assert_eq!(state.db().list_todays_invoices().unwrap().len(), 1);
    // The write queued for a later stage to send, in the same transaction.
    let queued = state.db().pending_sync_rows_for("invoices").unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].row_id, invoice.id);
}

/// The stage-2 worked example, priced through the same command the summary panel calls:
/// a 42U rack and five enterprise licences to an intra-state Tamil Nadu buyer.
#[test]
fn the_summary_panel_quote_matches_the_worked_example() {
    let quote = quote(
        "TN",
        &[
            QuoteLinePayload { qty: 1.0, rate: 45_000.0, tax_rate: 18.0 },
            QuoteLinePayload { qty: 5.0, rate: 12_000.0, tax_rate: 18.0 },
        ],
    );

    assert!(quote.intra_state);
    assert_eq!(quote.home_state, "TN");
    assert_eq!(quote.line_totals, vec![45_000.00, 60_000.00]);
    assert_eq!(quote.subtotal, 105_000.00);
    assert_eq!(quote.cgst, 9_450.00);
    assert_eq!(quote.sgst, 9_450.00);
    assert_eq!(quote.igst, 0.0);
    assert_eq!(quote.grand_total, 123_900.00);
}

#[test]
fn the_same_rows_billed_out_of_state_move_to_igst() {
    let quote = quote(
        "KA",
        &[
            QuoteLinePayload { qty: 1.0, rate: 45_000.0, tax_rate: 18.0 },
            QuoteLinePayload { qty: 5.0, rate: 12_000.0, tax_rate: 18.0 },
        ],
    );

    assert!(!quote.intra_state);
    assert_eq!(quote.cgst, 0.0);
    assert_eq!(quote.sgst, 0.0);
    assert_eq!(quote.igst, 18_900.00);
    assert_eq!(quote.grand_total, 123_900.00);
}

#[test]
fn removing_a_row_reprices_the_rest() {
    // Both rows, then the licence row alone — what the panel shows after a delete.
    let both = quote(
        "TN",
        &[
            QuoteLinePayload { qty: 1.0, rate: 45_000.0, tax_rate: 18.0 },
            QuoteLinePayload { qty: 5.0, rate: 12_000.0, tax_rate: 18.0 },
        ],
    );
    assert_eq!(both.grand_total, 123_900.00);

    let one = quote("TN", &[QuoteLinePayload { qty: 5.0, rate: 12_000.0, tax_rate: 18.0 }]);
    assert_eq!(one.line_totals, vec![60_000.00]);
    assert_eq!(one.subtotal, 60_000.00);
    assert_eq!(one.cgst, 5_400.00);
    assert_eq!(one.sgst, 5_400.00);
    assert_eq!(one.grand_total, 70_800.00);
}

#[test]
fn an_empty_or_half_typed_table_quotes_to_zero() {
    let empty = quote("TN", &[]);
    assert_eq!(empty.line_totals, Vec::<f64>::new());
    assert_eq!(empty.grand_total, 0.0);

    // A cleared qty box prices as zero rather than erroring the panel out.
    let blank_qty = quote("TN", &[QuoteLinePayload { qty: 0.0, rate: 45_000.0, tax_rate: 18.0 }]);
    assert_eq!(blank_qty.line_totals, vec![0.0]);
    assert_eq!(blank_qty.grand_total, 0.0);
}

#[test]
fn the_seeded_catalogue_carries_both_sample_items() {
    let (_dir, state) = console_state();

    let rack = state.db().search_item("RACK-42U-PRO").unwrap().remove(0);
    assert_eq!(rack.description, "42U Server Rack Pro");
    assert_eq!(rack.rate, 45_000.0);
    assert_eq!(rack.tax_rate, 18.0);

    let license = state.db().search_item("aBCOS").unwrap().remove(0);
    assert_eq!(license.item_code, "ABCOS-ENT-LIC");
    assert_eq!(license.description, "aBCOS Enterprise Lic");
    assert_eq!(license.rate, 12_000.0);

    // The customer the spec's example line shows.
    let ishta = state.db().search_customer("9600011223").unwrap().unwrap();
    assert_eq!(ishta.name, "Ishta Capital Investments");
    assert_eq!(ishta.gstin.as_deref(), Some("33AAAAA0000A1Z1"));
    assert_eq!(ishta.place_of_supply, "TN");
}

#[test]
fn the_new_customer_form_payload_registers_a_customer() {
    let (_dir, state) = console_state();

    // Exactly what invoke("create_customer", { payload }) sends.
    let json = r#"{
        "name": "Anand Electricals",
        "mobile": "9884455661",
        "gstin": "33AAFCA1234M1Z9",
        "place_of_supply": "TN"
    }"#;
    let payload: NewCustomerPayload = serde_json::from_str(json).unwrap();
    let created = state.db().create_customer(&NewCustomer::from(payload)).unwrap();

    assert_eq!(created.name, "Anand Electricals");
    assert_eq!(created.gstin.as_deref(), Some("33AAFCA1234M1Z9"));

    // Findable by the search that just missed, and queued for sync.
    assert_eq!(state.db().search_customer("9884455661").unwrap().unwrap(), created);
    let queued = state.db().pending_sync_rows_for("customers").unwrap();
    assert!(queued.iter().any(|r| r.row_id == created.id && r.op == "insert"));
}

/// The form leaves GSTIN empty for unregistered buyers; `null` must survive the trip.
#[test]
fn a_customer_without_a_gstin_round_trips() {
    let (_dir, state) = console_state();

    let json =
        r#"{ "name": "Walk-in", "mobile": "9112233445", "gstin": null, "place_of_supply": "TN" }"#;
    let payload: NewCustomerPayload = serde_json::from_str(json).unwrap();
    let created = state.db().create_customer(&NewCustomer::from(payload)).unwrap();

    assert_eq!(created.gstin, None);
}

/// The `invoice` object Print & Lock logs, captured verbatim from a run of the worked
/// example. Stage 3 hands exactly this to `create_invoice`, so it has to deserialize
/// into core's `NewInvoice` with nothing missing.
#[test]
fn the_logged_print_and_lock_payload_is_ready_for_create_invoice() {
    let logged = r#"{
        "customer_id": 4,
        "date": null,
        "payment_type": "cash",
        "lines": [
            { "item_id": 1, "qty": 1, "rate": 45000, "tax_rate": 18 },
            { "item_id": 2, "qty": 5, "rate": 12000, "tax_rate": 18 }
        ]
    }"#;

    let payload: NewInvoicePayload = serde_json::from_str(logged).expect("payload parses");
    let new_invoice = NewInvoice::from(payload);
    assert_eq!(new_invoice.customer_id, 4);
    assert_eq!(new_invoice.payment_type, "cash");
    assert_eq!(new_invoice.lines.len(), 2);

    // And it prices to the figures the panel displayed when it was logged.
    let (_dir, state) = console_state();
    let priced: Vec<QuoteLinePayload> = new_invoice
        .lines
        .iter()
        .map(|l| QuoteLinePayload {
            qty: l.qty,
            rate: l.rate.unwrap(),
            tax_rate: l.tax_rate.unwrap(),
        })
        .collect();
    let quote = quote("TN", &priced);
    assert_eq!(quote.subtotal, 105_000.00);
    assert_eq!(quote.grand_total, 123_900.00);

    // Saved against the seeded catalogue, core reaches the same numbers independently —
    // the ids in the payload are the seeded rack and licence.
    let saved = state.db().create_invoice(&new_invoice).unwrap();
    assert_eq!(saved.subtotal, 105_000.00);
    assert_eq!(saved.cgst, 9_450.00);
    assert_eq!(saved.sgst, 9_450.00);
    assert_eq!(saved.grand_total, 123_900.00);
}

fn worked_example(state: &AppState) -> NewInvoice {
    let rack = state.db().search_item("RACK-42U-PRO").unwrap().remove(0);
    let license = state.db().search_item("ABCOS-ENT-LIC").unwrap().remove(0);
    let customer_id = state.db().search_customer("9600011223").unwrap().unwrap().id;
    NewInvoice::from(NewInvoicePayload {
        customer_id,
        date: None,
        payment_type: "cash".into(),
        lines: vec![
            NewLinePayload {
                item_id: rack.id,
                qty: 1.0,
                rate: Some(45_000.0),
                tax_rate: Some(18.0),
            },
            NewLinePayload {
                item_id: license.id,
                qty: 5.0,
                rate: Some(12_000.0),
                tax_rate: Some(18.0),
            },
        ],
    })
}

/// Print & Lock: the invoice, both lines and the sync row all land, and the number is
/// allocated by the save rather than anything the screen held beforehand.
#[test]
fn saving_the_worked_example_persists_everything() {
    let (dir, state) = console_state();

    let example = worked_example(&state);
    let invoice = state.db().create_invoice(&example).unwrap();
    assert_eq!(invoice.invoice_no, "RI-2026-0001");
    assert_eq!(invoice.subtotal, 105_000.00);
    assert_eq!(invoice.cgst, 9_450.00);
    assert_eq!(invoice.sgst, 9_450.00);
    assert_eq!(invoice.grand_total, 123_900.00);
    assert_eq!(invoice.sync_status, "pending");

    let lines = state.db().invoice_lines(invoice.id).unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].line_total, 45_000.00);
    assert_eq!(lines[1].line_total, 60_000.00);

    let queued = state.db().pending_sync_rows_for("invoices").unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].row_id, invoice.id);
    assert_eq!(queued[0].op, "insert");
    assert!(queued[0].synced_at.is_none());
    assert_eq!(state.db().pending_sync_rows_for("invoice_lines").unwrap().len(), 2);

    // Reopened as a separate handle on the file, it is all still there.
    drop(state);
    let reopened = AppState::new(dir.path().join(DB_FILE_NAME)).unwrap();
    let stored = reopened.db().get_invoice(invoice.id).unwrap().unwrap();
    assert_eq!(stored.invoice_no, "RI-2026-0001");
    assert_eq!(stored.grand_total, 123_900.00);
}

/// New Transaction then a second, different invoice: the number increments and the first
/// invoice is untouched.
#[test]
fn a_second_invoice_takes_the_next_number() {
    let (_dir, state) = console_state();

    let example = worked_example(&state);
    let first = state.db().create_invoice(&example).unwrap();

    let cement = state.db().search_item("CEM-OPC-53").unwrap().remove(0);
    let buyer = state.db().search_customer("9791045678").unwrap().unwrap();
    let second = state
        .db()
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "upi".into(),
            created_by_user_id: None,
            lines: vec![NewInvoiceLine {
                item_id: cement.id,
                qty: 20.0,
                rate: None,
                tax_rate: None,
            }],
        })
        .unwrap();

    assert_eq!(first.invoice_no, "RI-2026-0001");
    assert_eq!(second.invoice_no, "RI-2026-0002");
    assert_ne!(first.id, second.id);

    // 20 x 410 = 8,200 @ 28% -> 1,148 each.
    assert_eq!(second.subtotal, 8_200.00);
    assert_eq!(second.cgst, 1_148.00);
    assert_eq!(second.grand_total, 10_496.00);

    // The first invoice kept its own lines and figures.
    assert_eq!(state.db().invoice_lines(first.id).unwrap().len(), 2);
    assert_eq!(state.db().get_invoice(first.id).unwrap().unwrap().grand_total, 123_900.00);
    assert_eq!(state.db().pending_sync_rows_for("invoices").unwrap().len(), 2);
    assert_eq!(state.db().list_todays_invoices().unwrap().len(), 2);
}

/// A failed save must leave nothing behind, so the screen can stay editable and the
/// operator can simply retry.
#[test]
fn a_rejected_save_writes_nothing() {
    let (_dir, state) = console_state();
    let buyer = state.db().search_customer("9600011223").unwrap().unwrap();

    let rejected = state.db().create_invoice(&NewInvoice {
        customer_id: buyer.id,
        date: None,
        payment_type: "cash".into(),
        created_by_user_id: None,
        lines: vec![NewInvoiceLine { item_id: 9_999, qty: 1.0, rate: None, tax_rate: None }],
    });
    assert!(rejected.is_err());

    assert!(state.db().list_todays_invoices().unwrap().is_empty());
    assert!(state.db().pending_sync_rows_for("invoices").unwrap().is_empty());
    assert!(state.db().pending_sync_rows_for("invoice_lines").unwrap().is_empty());

    // And the number was not burnt: the next good save still takes 0001.
    let example = worked_example(&state);
    let saved = state.db().create_invoice(&example).unwrap();
    assert_eq!(saved.invoice_no, "RI-2026-0001");
}

#[test]
fn totals_matching_the_screen_are_accepted() {
    let (_dir, state) = console_state();
    let expected = ExpectedTotals {
        subtotal: 105_000.00,
        cgst: 9_450.00,
        sgst: 9_450.00,
        igst: 0.0,
        grand_total: 123_900.00,
    };
    let example = worked_example(&state);
    assert!(check_expected_totals(&expected, &example, "TN", "TN").is_ok());
}

#[test]
fn totals_that_disagree_with_the_screen_are_refused() {
    let (_dir, state) = console_state();
    let invoice = worked_example(&state);

    // The panel showed a grand total short by a rupee.
    let stale = ExpectedTotals {
        subtotal: 105_000.00,
        cgst: 9_450.00,
        sgst: 9_450.00,
        igst: 0.0,
        grand_total: 123_899.00,
    };
    let refused = check_expected_totals(&stale, &invoice, "TN", "TN").unwrap_err();
    assert!(refused.contains("grand total"), "{refused}");

    // Billing the same rows out of state moves the tax to IGST, so an intra-state
    // expectation no longer matches.
    let intra = ExpectedTotals {
        subtotal: 105_000.00,
        cgst: 9_450.00,
        sgst: 9_450.00,
        igst: 0.0,
        grand_total: 123_900.00,
    };
    assert!(check_expected_totals(&intra, &invoice, "KA", "TN").is_err());
}

/// Rows that defer to the item master are priced inside core; the guard skips them
/// rather than keeping a second copy of core's pricing rules.
#[test]
fn rows_without_explicit_prices_skip_the_guard() {
    let (_dir, state) = console_state();
    let cement = state.db().search_item("CEM-OPC-53").unwrap().remove(0);
    let deferred = NewInvoice {
        customer_id: 1,
        date: None,
        payment_type: "cash".into(),
        created_by_user_id: None,
        lines: vec![NewInvoiceLine { item_id: cement.id, qty: 1.0, rate: None, tax_rate: None }],
    };

    let nonsense =
        ExpectedTotals { subtotal: 1.0, cgst: 1.0, sgst: 1.0, igst: 1.0, grand_total: 1.0 };
    assert!(check_expected_totals(&nonsense, &deferred, "TN", "TN").is_ok());
}

/// Bills four invoices through the console the way stage 3 saves them.
fn billed_history(state: &AppState) -> Vec<realinvoice_core::Invoice> {
    let example = worked_example(state);
    let first = state.db().create_invoice(&example).unwrap();

    let cement = state.db().search_item("CEM-OPC-53").unwrap().remove(0);
    let kaveri = state.db().search_customer("9791045678").unwrap().unwrap().id;
    let second = NewInvoice {
        customer_id: kaveri,
        date: None,
        payment_type: "upi".into(),
        created_by_user_id: None,
        lines: vec![NewInvoiceLine { item_id: cement.id, qty: 20.0, rate: None, tax_rate: None }],
    };
    let second = state.db().create_invoice(&second).unwrap();

    let license = state.db().search_item("ABCOS-ENT-LIC").unwrap().remove(0);
    let deccan = state.db().search_customer("9845567890").unwrap().unwrap().id;
    let third = NewInvoice {
        customer_id: deccan,
        date: None,
        payment_type: "credit".into(),
        created_by_user_id: None,
        lines: vec![NewInvoiceLine { item_id: license.id, qty: 2.0, rate: None, tax_rate: None }],
    };
    let third = state.db().create_invoice(&third).unwrap();

    // One on an older date, so a "today" filter has something to exclude.
    let pipe = state.db().search_item("PVC-PIPE-4").unwrap().remove(0);
    let older = NewInvoice {
        customer_id: kaveri,
        date: Some("2026-08-20".into()),
        payment_type: "cash".into(),
        created_by_user_id: None,
        lines: vec![NewInvoiceLine { item_id: pipe.id, qty: 4.0, rate: None, tax_rate: None }],
    };
    let older = state.db().create_invoice(&older).unwrap();

    vec![first, second, third, older]
}

#[test]
fn the_history_pane_lists_every_billed_invoice_with_its_totals() {
    let (_dir, state) = console_state();
    let billed = billed_history(&state);

    let all = state.db().list_invoices(&InvoiceFilter::default()).unwrap();
    assert_eq!(all.len(), 4);

    // Every saved invoice is listed, with the total it was saved with.
    for invoice in &billed {
        let listed = all
            .iter()
            .find(|s| s.invoice.id == invoice.id)
            .unwrap_or_else(|| panic!("{} missing from history", invoice.invoice_no));
        assert_eq!(listed.invoice.grand_total, invoice.grand_total);
        assert_eq!(listed.invoice.invoice_no, invoice.invoice_no);
        assert!(!listed.customer_name.is_empty());
        assert!(!listed.invoice.created_at.is_empty(), "the list shows a date and time");
    }
}

#[test]
fn todays_filter_excludes_an_older_invoice_and_a_custom_range_finds_it() {
    let (_dir, state) = console_state();
    let billed = billed_history(&state);
    let older = billed.last().unwrap();
    let today = realinvoice_core::Invoice::clone(&billed[0]).date;

    let todays = state
        .db()
        .list_invoices(&InvoiceFilter {
            from: Some(today.clone()),
            to: Some(today),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(todays.len(), 3);
    assert!(todays.iter().all(|s| s.invoice.id != older.id));

    // A custom range around August finds only the older one.
    let august = state
        .db()
        .list_invoices(&InvoiceFilter {
            from: Some("2026-08-01".into()),
            to: Some("2026-08-31".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(august.len(), 1);
    assert_eq!(august[0].invoice.id, older.id);

    // And a range containing nothing narrows to nothing, without erroring.
    assert!(state
        .db()
        .list_invoices(&InvoiceFilter {
            from: Some("2020-01-01".into()),
            to: Some("2020-12-31".into()),
            ..Default::default()
        })
        .unwrap()
        .is_empty());
}

#[test]
fn the_detail_view_matches_exactly_what_was_saved() {
    let (_dir, state) = console_state();
    let billed = billed_history(&state);
    let saved = &billed[0]; // the worked example

    let detail = state.db().get_invoice_detail(saved.id).unwrap().expect("detail");

    // Header figures are the stored ones, not recomputed for display.
    assert_eq!(&detail.invoice, saved);
    assert_eq!(detail.invoice.grand_total, 123_900.00);
    assert_eq!(detail.customer.name, "Ishta Capital Investments");

    // Every line matches the stored row, and they sum to the stored subtotal.
    let stored_lines = state.db().invoice_lines(saved.id).unwrap();
    assert_eq!(detail.lines.len(), stored_lines.len());
    for (shown, stored) in detail.lines.iter().zip(&stored_lines) {
        assert_eq!(&shown.line, stored);
    }
    let summed: f64 = detail.lines.iter().map(|l| l.line.line_total).sum();
    assert_eq!(summed, detail.invoice.subtotal);
}

/// Reprint renders from the stored invoice, so its numbers are the saved ones with
/// nothing re-entered and nothing recomputed.
#[test]
fn reprinting_reads_the_same_figures_back() {
    let (dir, state) = console_state();
    let billed = billed_history(&state);
    let saved = billed[0].clone();
    drop(state);

    // A fresh process, as a reprint days later would be.
    let reopened = AppState::new(dir.path().join(DB_FILE_NAME)).unwrap();
    let detail = reopened.db().get_invoice_detail(saved.id).unwrap().unwrap();

    assert_eq!(detail.invoice.invoice_no, saved.invoice_no);
    assert_eq!(detail.invoice.subtotal, saved.subtotal);
    assert_eq!(detail.invoice.cgst, saved.cgst);
    assert_eq!(detail.invoice.sgst, saved.sgst);
    assert_eq!(detail.invoice.igst, saved.igst);
    assert_eq!(detail.invoice.grand_total, saved.grand_total);
    assert_eq!(detail.invoice.created_at, saved.created_at);

    // The sheet prints code, description and UOM, so the detail has to carry them.
    assert_eq!(detail.lines[0].item_code, "RACK-42U-PRO");
    assert_eq!(detail.lines[0].description, "42U Server Rack Pro");
    assert_eq!(detail.lines[0].uom, "NOS");
}

#[test]
fn searching_history_by_customer_or_number_narrows_the_list() {
    let (_dir, state) = console_state();
    billed_history(&state);

    let by_name = state
        .db()
        .list_invoices(&InvoiceFilter { text: Some("ishta".into()), ..Default::default() })
        .unwrap();
    assert_eq!(by_name.len(), 1);
    assert_eq!(by_name[0].invoice.invoice_no, "RI-2026-0001");

    let by_number = state
        .db()
        .list_invoices(&InvoiceFilter { text: Some("0002".into()), ..Default::default() })
        .unwrap();
    assert_eq!(by_number.len(), 1);
    assert_eq!(by_number[0].customer_name, "Kaveri Hardware");
}

// ------------------------------------------------------------------ sign-in

/// Creates the owner exactly as `create_first_user` does when somebody fills in the
/// setup screen. Nothing else can put the first row in the users table.
fn set_up_owner(state: &AppState) -> User {
    let mut db = state.db();
    db.create_user(
        &NewUser {
            username: "priya".into(),
            display_name: "Priya Raman".into(),
            role: Role::Owner,
        },
        "counter-top-2026",
    )
    .unwrap()
}

#[test]
fn a_fresh_console_has_no_account_and_asks_to_be_set_up() {
    let (_dir, state) = console_state();

    // Demo customers and items are seeded; an account is not. `auth_status` reports this
    // as `needs_setup`, and the shell shows "Create your account" instead of the gate.
    assert_eq!(state.db().count_users().unwrap(), 0);
    assert_eq!(state.db().attempt_login("admin", "admin").unwrap(), LoginOutcome::Invalid);
    assert!(state.session().is_none());
}

#[test]
fn a_restart_keeps_the_account_and_does_not_ask_to_set_up_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);

    let first = AppState::new(path.clone()).unwrap();
    set_up_owner(&first);
    drop(first);

    let restarted = AppState::new(path).unwrap();
    assert_eq!(restarted.db().count_users().unwrap(), 1, "setup happens once");
    assert!(matches!(
        restarted.db().attempt_login("priya", "counter-top-2026").unwrap(),
        LoginOutcome::Ok(_)
    ));
    assert!(restarted.session().is_none(), "a restart signs everybody out");
}

/// Managing accounts is owner-only in Rust, not just in the sidebar. A cashier who
/// reaches the command directly — the shell reloaded, a crafted `invoke` — is refused.
#[test]
fn only_an_owner_can_reach_the_account_commands() {
    let (_dir, state) = console_state();
    let owner = set_up_owner(&state);
    let cashier = state
        .db()
        .create_user(
            &NewUser {
                username: "meena".into(),
                display_name: "Meena R".into(),
                role: Role::Cashier,
            },
            "counter-password",
        )
        .unwrap();

    let as_owner = Session { token: "t".into(), user: owner };
    let as_cashier = Session { token: "t".into(), user: cashier };

    assert!(require_owner(Some(as_owner.clone())).is_ok());
    assert!(require_session(Some(as_cashier.clone())).is_ok(), "a cashier can still bill");
    assert!(require_owner(Some(as_cashier)).is_err(), "but cannot manage accounts");
    assert!(require_owner(None).is_err(), "and neither can a caller with no session");
    assert!(require_session(None).is_err());
}

#[test]
fn a_session_starts_empty_and_clears_on_sign_out() {
    let (_dir, state) = console_state();
    assert!(state.session().is_none(), "nothing is reachable before sign-in");

    let owner = set_up_owner(&state);
    state.begin_session(Session { token: "token-1".into(), user: owner.clone() });

    let live = state.session().expect("signed in");
    assert_eq!(live.user.id, owner.id);
    assert_eq!(live.token, "token-1");

    state.end_session();
    assert!(state.session().is_none(), "sign-out leaves nothing behind");
}

#[test]
fn an_invoice_is_attributed_to_whoever_is_signed_in() {
    let (_dir, state) = console_state();
    let owner = set_up_owner(&state);

    let cashier = state
        .db()
        .create_user(
            &NewUser {
                username: "meena".into(),
                display_name: "Meena R".into(),
                role: Role::Cashier,
            },
            "counter-password",
        )
        .unwrap();

    // Two invoices, billed by two different people.
    let mut example = worked_example(&state);
    example.created_by_user_id = Some(owner.id);
    let by_owner = state.db().create_invoice(&example).unwrap();

    let mut second = worked_example(&state);
    second.created_by_user_id = Some(cashier.id);
    let by_cashier = state.db().create_invoice(&second).unwrap();

    assert_eq!(by_owner.created_by_user_id, Some(owner.id));
    assert_eq!(by_cashier.created_by_user_id, Some(cashier.id));

    // The history list names each biller.
    let listed = state.db().list_invoices(&InvoiceFilter::default()).unwrap();
    let named: Vec<Option<&str>> = listed.iter().map(|s| s.created_by.as_deref()).collect();
    assert!(named.contains(&Some("Priya Raman")));
    assert!(named.contains(&Some("Meena R")));
}

/// The payload type carries no attribution field at all, so a caller cannot bill as
/// somebody else — `create_invoice` fills it from the session.
#[test]
fn attribution_cannot_be_supplied_by_the_caller() {
    let json = r#"{
        "customer_id": 1,
        "payment_type": "upi",
        "created_by_user_id": 99,
        "lines": []
    }"#;

    let payload: NewInvoicePayload = serde_json::from_str(json).unwrap();
    let new_invoice = NewInvoice::from(payload);
    assert_eq!(new_invoice.created_by_user_id, None, "the smuggled id is dropped");
}

#[test]
fn each_payment_type_is_saved_as_selected() {
    let (_dir, state) = console_state();
    let owner = set_up_owner(&state);

    for chosen in ["upi", "cash", "card"] {
        let json = format!(r#"{{ "customer_id": 1, "payment_type": "{chosen}", "lines": [] }}"#);
        let payload: NewInvoicePayload = serde_json::from_str(&json).unwrap();

        let mut new_invoice = NewInvoice::from(payload);
        new_invoice.created_by_user_id = Some(owner.id);
        new_invoice.customer_id = state.db().search_customer("9600011223").unwrap().unwrap().id;
        let rack = state.db().search_item("RACK-42U-PRO").unwrap().remove(0);
        new_invoice.lines =
            vec![NewInvoiceLine { item_id: rack.id, qty: 1.0, rate: None, tax_rate: None }];

        let saved = state.db().create_invoice(&new_invoice).unwrap();
        assert_eq!(saved.payment_type, chosen);

        let stored = state.db().get_invoice(saved.id).unwrap().unwrap();
        assert_eq!(stored.payment_type, chosen);
        assert_eq!(stored.created_by_user_id, Some(owner.id));
    }
}

// ------------------------------------------------------------------ inventory

fn item_payload(rate: f64, tax_rate: f64, uom: &str) -> NewItemPayload {
    NewItemPayload {
        item_code: "  patch-cat6  ".into(),
        description: "  Cat6 patch cable 2m  ".into(),
        rate,
        tax_rate,
        uom: uom.into(),
    }
}

#[test]
fn an_item_from_the_form_is_trimmed_and_rounded() {
    let checked = check_item(&item_payload(180.005, 18.0, " NOS ")).unwrap();

    assert_eq!(checked.item_code, "patch-cat6", "the surrounding spaces go");
    assert_eq!(checked.description, "Cat6 patch cable 2m");
    assert_eq!(checked.uom, "NOS");
    // Rounded to paise by the same function every other amount goes through, so a price
    // cannot enter the catalogue carrying a fraction that resurfaces inside a total.
    assert_eq!(checked.rate, 180.01);
    assert_eq!(checked.tax_rate, 18.0);
}

#[test]
fn an_item_with_no_unit_gets_one() {
    // A unit is never blank on a printed bill; NOS is what a counter means by "each".
    assert_eq!(check_item(&item_payload(180.0, 18.0, "")).unwrap().uom, "NOS");
    assert_eq!(check_item(&item_payload(180.0, 18.0, "   ")).unwrap().uom, "NOS");
    assert_eq!(check_item(&item_payload(180.0, 18.0, "BAG")).unwrap().uom, "BAG");
}

#[test]
fn a_nonsense_price_or_tax_rate_is_refused() {
    // A negative price is a typo, and the person who made it is standing at the till.
    assert!(check_item(&item_payload(-1.0, 18.0, "NOS")).is_err());
    assert!(check_item(&item_payload(f64::NAN, 18.0, "NOS")).is_err());
    assert!(check_item(&item_payload(f64::INFINITY, 18.0, "NOS")).is_err());

    // GST has no rate above 100%, and a stray digit here would misprice every future bill.
    assert!(check_item(&item_payload(180.0, 900.0, "NOS")).is_err());
    assert!(check_item(&item_payload(180.0, -5.0, "NOS")).is_err());
    assert!(check_item(&item_payload(180.0, f64::NAN, "NOS")).is_err());

    // The boundaries themselves are legitimate: zero-rated goods, and free samples.
    assert!(check_item(&item_payload(0.0, 0.0, "NOS")).is_ok());
    assert!(check_item(&item_payload(180.0, 100.0, "NOS")).is_ok());
}

#[test]
fn analytics_figures_come_from_core_and_reconcile() {
    let (_dir, state) = console_state();
    let biller = set_up_owner(&state);

    // Two invoices, so every aggregate folds more than one row.
    for pay in ["cash", "upi"] {
        let mut invoice = worked_example(&state);
        invoice.payment_type = pay.into();
        invoice.created_by_user_id = Some(biller.id);
        state.db().create_invoice(&invoice).unwrap();
    }

    let db = state.db();
    let all = db.sales_summary(&DateRange::default()).unwrap();
    assert_eq!(all.invoice_count, 2);
    // The worked example is the ₹1,05,000 bill that totals ₹1,23,900, twice over.
    assert_eq!(all.subtotal, 210_000.0);
    assert_eq!(all.grand_total, 247_800.0);
    assert_eq!(all.tax_total, 37_800.0);
    assert_eq!(all.subtotal + all.tax_total, all.grand_total, "the parts make the whole");

    // Each breakdown covers the same invoices exactly once.
    let mix = db.payment_mix(&DateRange::default()).unwrap();
    assert_eq!(mix.iter().map(|m| m.invoice_count).sum::<i64>(), all.invoice_count);
    assert_eq!(mix.iter().map(|m| m.grand_total).sum::<f64>(), all.grand_total);

    let days = db.daily_totals(&DateRange::default()).unwrap();
    assert_eq!(days.iter().map(|d| d.grand_total).sum::<f64>(), all.grand_total);

    assert!(!db.top_items(&DateRange::default(), 8).unwrap().is_empty());
}

// ------------------------------------------------------- login rate limiting

#[test]
fn the_lockout_wait_is_described_in_words_a_person_would_use() {
    // Rounded up throughout: being told "1 minute" and still being refused at 55 seconds
    // reads as the app lying.
    assert_eq!(describe_wait(0), "less than a minute");
    assert_eq!(describe_wait(45), "less than a minute");
    assert_eq!(describe_wait(60), "less than a minute");
    assert_eq!(describe_wait(61), "2 minutes");
    assert_eq!(describe_wait(90), "2 minutes");
    assert_eq!(describe_wait(120), "2 minutes");
    assert_eq!(describe_wait(121), "3 minutes");
    assert_eq!(describe_wait(15 * 60), "15 minutes");

    // And the lockout length is quoted from core, so the screen cannot promise a window
    // the limiter does not keep.
    assert_eq!(
        describe_window(),
        if realinvoice_core::LOGIN_WINDOW_MINUTES == 1 {
            "a minute".to_string()
        } else {
            format!("{} minutes", realinvoice_core::LOGIN_WINDOW_MINUTES)
        }
    );
}

#[test]
fn the_console_signs_in_through_the_rate_limited_path() {
    let (_dir, state) = console_state();
    set_up_owner(&state);

    // Five failures through the same call the command makes.
    for _ in 0..5 {
        assert_eq!(state.db().attempt_login("priya", "wrong").unwrap(), LoginOutcome::Invalid);
    }

    // And the sixth is refused with the correct password. If the command layer had kept
    // its own credential check, this is where that would show up.
    assert!(matches!(
        state.db().attempt_login("priya", "counter-top-2026").unwrap(),
        LoginOutcome::LockedOut(_)
    ));

    // A different account on the same till is untouched.
    state
        .db()
        .create_user(
            &NewUser {
                username: "meena".into(),
                display_name: "Meena R".into(),
                role: Role::Cashier,
            },
            "counter-password",
        )
        .unwrap();
    assert!(matches!(
        state.db().attempt_login("meena", "counter-password").unwrap(),
        LoginOutcome::Ok(_)
    ));
}

#[test]
fn a_lockout_survives_restarting_the_app() {
    // The limiter is rows in SQLite, not memory, so closing the app is not a way out of
    // it. That is the obvious bypass to try.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);

    let first = AppState::new(path.clone()).unwrap();
    set_up_owner(&first);
    for _ in 0..5 {
        first.db().attempt_login("priya", "wrong").unwrap();
    }
    assert!(first.db().lockout_for("priya").unwrap().is_some());
    drop(first);

    let restarted = AppState::new(path).unwrap();
    assert!(
        restarted.db().lockout_for("priya").unwrap().is_some(),
        "a restart does not clear the lockout"
    );
    assert!(matches!(
        restarted.db().attempt_login("priya", "counter-top-2026").unwrap(),
        LoginOutcome::LockedOut(_)
    ));
}

// ------------------------------------------------------------- credit notes

#[test]
fn only_an_owner_can_reverse_an_invoice() {
    // The same gate the Users screen uses, for the same reason: a cashier who could void
    // their own sales is the shape of most till fraud. Hiding the button is the courtesy;
    // this is the control.
    let (_dir, state) = console_state();
    let owner = set_up_owner(&state);
    let cashier = state
        .db()
        .create_user(
            &NewUser {
                username: "meena".into(),
                display_name: "Meena R".into(),
                role: Role::Cashier,
            },
            "counter-password",
        )
        .unwrap();

    let as_owner = Session { token: "t".into(), user: owner };
    let as_cashier = Session { token: "t".into(), user: cashier };

    assert!(require_owner(Some(as_owner)).is_ok());
    assert!(
        require_session(Some(as_cashier.clone())).is_ok(),
        "a cashier can still bill and read invoices"
    );
    assert!(require_owner(Some(as_cashier)).is_err(), "but cannot issue a credit note");
    assert!(require_owner(None).is_err());
}

#[test]
fn a_credit_note_is_attributed_to_the_session_not_the_payload() {
    let (_dir, state) = console_state();
    let owner = set_up_owner(&state);

    let mut invoice = worked_example(&state);
    invoice.created_by_user_id = Some(owner.id);
    let invoice = state.db().create_invoice(&invoice).unwrap();

    let lines = state.db().creditable_lines(invoice.id).unwrap();
    assert!(!lines.is_empty());

    // The payload type carries no attribution field at all, so a caller cannot record a
    // reversal as somebody else's decision — the same rule invoices follow.
    let json = format!(
        r#"{{ "original_invoice_id": {}, "reason": "returned",
              "created_by_user_id": 99,
              "lines": [{{ "invoice_line_id": {}, "qty": 1 }}] }}"#,
        invoice.id, lines[0].invoice_line_id
    );
    let payload: NewCreditNotePayload = serde_json::from_str(&json).unwrap();
    assert_eq!(payload.original_invoice_id, invoice.id);
    assert_eq!(payload.lines.len(), 1);

    // Issued the way the command does it: attribution from the session.
    let note = state
        .db()
        .create_credit_note(&NewCreditNote {
            original_invoice_id: payload.original_invoice_id,
            reason: payload.reason,
            date: None,
            created_by_user_id: Some(owner.id),
            lines: payload
                .lines
                .into_iter()
                .map(|l| NewCreditNoteLine { invoice_line_id: l.invoice_line_id, qty: l.qty })
                .collect(),
        })
        .unwrap();

    assert_eq!(note.created_by_user_id, Some(owner.id), "the smuggled id is not used");
    assert_eq!(note.credit_note_no, "CN-2026-0001");

    // The invoice is untouched, which is the point of the whole design.
    assert_eq!(state.db().get_invoice(invoice.id).unwrap().unwrap(), invoice);
}

/// A one-off typed onto a bill is billable but never joins the catalogue.
///
/// This is the whole point of the flag. A counter sells a repair charge or a loose
/// fitting every other day; if each one landed in Inventory, the list the shop actually
/// maintains would be unusable inside a month.
#[test]
fn a_one_off_item_is_billable_but_stays_out_of_the_catalogue() {
    let (_dir, state) = console_state();

    let before = state.db().count_items().unwrap();
    let one_off = state
        .db()
        .create_custom_item("Site visit and rack re-dress", 2_500.0, 18.0, "NOS")
        .unwrap();

    assert!(one_off.custom, "the flag is what keeps it out of the lists");
    assert_eq!(state.db().count_items().unwrap(), before, "Inventory is unchanged");

    // Not by code, not by description, not by an empty query that lists everything.
    assert!(state.db().search_item("Site visit").unwrap().is_empty());
    assert!(state.db().search_item(&one_off.item_code).unwrap().is_empty());
    assert!(state.db().search_item("").unwrap().iter().all(|item| item.id != one_off.id));

    // But it is a real row, so a line can point at it and the invoice prices normally.
    let customer_id = state.db().search_customer("9600011223").unwrap().unwrap().id;
    let invoice = state
        .db()
        .create_invoice(&NewInvoice::from(NewInvoicePayload {
            customer_id,
            date: None,
            payment_type: "cash".into(),
            lines: vec![NewLinePayload {
                item_id: one_off.id,
                qty: 2.0,
                rate: Some(2_500.0),
                tax_rate: Some(18.0),
            }],
        }))
        .unwrap();

    assert_eq!(invoice.subtotal, 5_000.00);
    assert_eq!(invoice.grand_total, 5_900.00);

    // And the detail view can tell the line apart from a catalogue one.
    let detail = state.db().get_invoice_detail(invoice.id).unwrap().unwrap();
    assert!(detail.lines[0].custom);
    assert_eq!(detail.lines[0].description, "Site visit and rack re-dress");
}

#[test]
fn a_one_off_with_nonsense_numbers_is_refused() {
    let (_dir, state) = console_state();

    assert!(state.db().create_custom_item("   ", 100.0, 18.0, "NOS").is_err());
    assert!(state.db().create_custom_item("Delivery", -1.0, 18.0, "NOS").is_err());
    assert!(state.db().create_custom_item("Delivery", f64::NAN, 18.0, "NOS").is_err());
    assert!(state.db().create_custom_item("Delivery", 100.0, 101.0, "NOS").is_err());
    assert!(state.db().create_custom_item("Delivery", 100.0, -1.0, "NOS").is_err());

    // A missing unit is filled in rather than refused, as it is on the Inventory form.
    let filled = state.db().create_custom_item("Delivery", 100.0, 18.0, "").unwrap();
    assert_eq!(filled.uom, "NOS");
}

/// Clearing the demo data must never reach anything an invoice points at.
#[test]
fn clearing_demo_data_keeps_everything_already_billed() {
    let (_dir, state) = console_state();

    // Bill the worked example, so one customer and two items are now in the books.
    let example = worked_example(&state);
    let invoice = state.db().create_invoice(&example).unwrap();
    let billed_customer = invoice.customer_id;
    let billed_items: Vec<i64> =
        state.db().invoice_lines(invoice.id).unwrap().iter().map(|line| line.item_id).collect();
    assert_eq!(billed_items.len(), 2);

    let cleared = state.db().clear_demo_data().unwrap();

    assert_eq!(cleared.items_kept, 2, "both billed items survive");
    assert_eq!(cleared.customers_kept, 1, "so does the customer they were billed to");
    assert!(cleared.items_removed > 0, "the untouched seed rows go");
    assert!(cleared.customers_removed > 0);

    // The invoice itself is untouched, and still reads end to end.
    let detail = state.db().get_invoice_detail(invoice.id).unwrap().unwrap();
    assert_eq!(detail.invoice.grand_total, 123_900.00);
    assert_eq!(detail.lines.len(), 2);
    assert_eq!(detail.customer.id, billed_customer);
    for item_id in &billed_items {
        assert!(state.db().get_item(*item_id).unwrap().is_some());
    }

    // And what it removed is really gone from the catalogue.
    assert_eq!(state.db().count_items().unwrap(), 2);

    // Running it twice is not an error; the second pass simply finds nothing left.
    let again = state.db().clear_demo_data().unwrap();
    assert_eq!(again.items_removed, 0);
    assert_eq!(again.customers_removed, 0);
    assert_eq!(again.items_kept, 2);
}

#[test]
fn only_an_owner_can_clear_the_demo_data() {
    // It is destructive and one-way. A cashier clearing the catalogue mid-shift, by
    // accident or otherwise, is not something to leave to a hidden button.
    let (_dir, state) = console_state();
    let owner = set_up_owner(&state);
    let cashier = state
        .db()
        .create_user(
            &NewUser {
                username: "meena".into(),
                display_name: "Meena R".into(),
                role: Role::Cashier,
            },
            "counter-password",
        )
        .unwrap();

    assert!(require_owner(Some(Session { token: "t".into(), user: owner })).is_ok());
    assert!(
        require_session(Some(Session { token: "t".into(), user: cashier.clone() })).is_ok(),
        "a cashier can still add a one-off item to their own bill"
    );
    assert!(require_owner(Some(Session { token: "t".into(), user: cashier })).is_err());
    assert!(require_owner(None).is_err());
}
