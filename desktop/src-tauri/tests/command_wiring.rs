//! The Office Console's command layer, exercised without a window.
//!
//! Tauri commands can't be called outside a running app, so these tests drive the state
//! and payload conversions that sit behind them — the parts this crate actually owns.
//! What they prove: the console opens its own database file, seeds itself, and a frontend
//! payload survives the trip into core unchanged.

use realinvoice_core::{NewInvoice, NewInvoiceLine};
use realinvoice_desktop_lib::commands::{NewInvoicePayload, NewLinePayload};
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
    assert_eq!(state.db().search_item("").unwrap().len(), 5);
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
