//! The Office Console's command layer, exercised without a window.
//!
//! Tauri commands can't be called outside a running app, so these tests drive the state
//! and payload conversions that sit behind them — the parts this crate actually owns.
//! What they prove: the console opens its own database file, seeds itself, and a frontend
//! payload survives the trip into core unchanged.

use realinvoice_core::{seed, NewCustomer, NewInvoice, NewInvoiceLine};
use realinvoice_desktop_lib::commands::{
    check_expected_totals_for_test as check_expected_totals, quote_invoice, ExpectedTotals,
    NewCustomerPayload, NewInvoicePayload, NewLinePayload, QuoteLinePayload,
};
use realinvoice_desktop_lib::state::{AppState, DB_FILE_NAME};

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
    let quote = quote_invoice(
        "TN".into(),
        vec![
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
    let quote = quote_invoice(
        "KA".into(),
        vec![
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
    let both = quote_invoice(
        "TN".into(),
        vec![
            QuoteLinePayload { qty: 1.0, rate: 45_000.0, tax_rate: 18.0 },
            QuoteLinePayload { qty: 5.0, rate: 12_000.0, tax_rate: 18.0 },
        ],
    );
    assert_eq!(both.grand_total, 123_900.00);

    let one = quote_invoice(
        "TN".into(),
        vec![QuoteLinePayload { qty: 5.0, rate: 12_000.0, tax_rate: 18.0 }],
    );
    assert_eq!(one.line_totals, vec![60_000.00]);
    assert_eq!(one.subtotal, 60_000.00);
    assert_eq!(one.cgst, 5_400.00);
    assert_eq!(one.sgst, 5_400.00);
    assert_eq!(one.grand_total, 70_800.00);
}

#[test]
fn an_empty_or_half_typed_table_quotes_to_zero() {
    let empty = quote_invoice("TN".into(), vec![]);
    assert_eq!(empty.line_totals, Vec::<f64>::new());
    assert_eq!(empty.grand_total, 0.0);

    // A cleared qty box prices as zero rather than erroring the panel out.
    let blank_qty = quote_invoice(
        "TN".into(),
        vec![QuoteLinePayload { qty: 0.0, rate: 45_000.0, tax_rate: 18.0 }],
    );
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
    let quote = quote_invoice(
        "TN".into(),
        new_invoice
            .lines
            .iter()
            .map(|l| QuoteLinePayload {
                qty: l.qty,
                rate: l.rate.unwrap(),
                tax_rate: l.tax_rate.unwrap(),
            })
            .collect(),
    );
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
        lines: vec![NewInvoiceLine { item_id: cement.id, qty: 1.0, rate: None, tax_rate: None }],
    };

    let nonsense =
        ExpectedTotals { subtotal: 1.0, cgst: 1.0, sgst: 1.0, igst: 1.0, grand_total: 1.0 };
    assert!(check_expected_totals(&nonsense, &deferred, "TN", "TN").is_ok());
}
