//! End-to-end tests: an invoice goes in through `Db`, and comes back out of a freshly
//! reopened SQLite file with its lines and its `sync_queue` rows intact.

use realinvoice_core::{seed, CoreError, Db, NewCustomer, NewInvoice, NewInvoiceLine, NewItem};

fn seeded_db() -> Db {
    let mut db = Db::open_in_memory().expect("open in-memory db");
    seed::seed_demo_data(&mut db).expect("seed");
    db
}

fn customer(db: &Db, mobile: &str) -> realinvoice_core::Customer {
    db.search_customer(mobile).unwrap().unwrap_or_else(|| panic!("no customer {mobile}"))
}

fn item(db: &Db, code: &str) -> realinvoice_core::Item {
    db.search_item(code).unwrap().into_iter().next().unwrap_or_else(|| panic!("no item {code}"))
}

#[test]
fn an_invoice_can_be_created_end_to_end() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let cement = item(&db, "CEM-OPC-53");
    let steel = item(&db, "TMT-12MM");

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: Some("2026-09-11".into()),
            payment_type: "cash".into(),
            lines: vec![
                NewInvoiceLine { item_id: cement.id, qty: 10.0, rate: None, tax_rate: None },
                NewInvoiceLine { item_id: steel.id, qty: 5.0, rate: None, tax_rate: None },
            ],
        })
        .expect("create invoice");

    // 10 x 410 = 4,100 @ 28% -> 574 + 574. 5 x 620 = 3,100 @ 18% -> 279 + 279.
    assert_eq!(invoice.invoice_no, "RI-2026-0001");
    assert_eq!(invoice.subtotal, 7_200.00);
    assert_eq!(invoice.cgst, 853.00);
    assert_eq!(invoice.sgst, 853.00);
    assert_eq!(invoice.igst, 0.0);
    assert_eq!(invoice.grand_total, 8_906.00);
    assert_eq!(invoice.sync_status, "pending");

    let lines = db.invoice_lines(invoice.id).unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].line_total, 4_100.00);
    assert_eq!(lines[1].line_total, 3_100.00);

    assert_eq!(db.list_invoices_for_date("2026-09-11".parse().unwrap()).unwrap().len(), 1);
}

#[test]
fn sync_queue_picks_up_the_invoice_and_every_line() {
    let mut db = Db::open_in_memory().unwrap();
    let buyer = db
        .upsert_customer(&NewCustomer {
            name: "Sri Balaji Traders".into(),
            gstin: Some("33AABCS1429B1ZP".into()),
            place_of_supply: "TN".into(),
            mobile: "9840012345".into(),
        })
        .unwrap();
    let widget = db
        .upsert_item(&NewItem {
            item_code: "W-1".into(),
            description: "Widget".into(),
            rate: 100.0,
            tax_rate: 18.0,
            uom: "NOS".into(),
        })
        .unwrap();

    // The customer and item writes queue first.
    assert_eq!(db.pending_sync_rows_for("customers").unwrap().len(), 1);
    assert_eq!(db.pending_sync_rows_for("items").unwrap().len(), 1);

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "upi".into(),
            lines: vec![
                NewInvoiceLine { item_id: widget.id, qty: 2.0, rate: None, tax_rate: None },
                NewInvoiceLine { item_id: widget.id, qty: 3.0, rate: Some(90.0), tax_rate: None },
            ],
        })
        .unwrap();

    let queued_invoices = db.pending_sync_rows_for("invoices").unwrap();
    assert_eq!(queued_invoices.len(), 1);
    let row = &queued_invoices[0];
    assert_eq!(row.row_id, invoice.id);
    assert_eq!(row.op, "insert");
    assert!(row.synced_at.is_none(), "nothing drains the queue yet");

    // The payload is the invoice itself, so a later stage can send it without a re-read.
    let payload: serde_json::Value = serde_json::from_str(&row.payload_json).unwrap();
    assert_eq!(payload["invoice_no"], invoice.invoice_no.as_str());
    assert_eq!(payload["grand_total"], invoice.grand_total);

    let queued_lines = db.pending_sync_rows_for("invoice_lines").unwrap();
    assert_eq!(queued_lines.len(), 2);
    assert!(queued_lines.iter().all(|r| r.op == "insert"));
}

#[test]
fn an_invoice_survives_closing_and_reopening_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite");

    let (invoice_id, invoice_no) = {
        let mut db = Db::open(&path).unwrap();
        seed::seed_demo_data(&mut db).unwrap();
        let buyer = customer(&db, "9791045678");
        let pipe = item(&db, "PVC-PIPE-4");
        let invoice = db
            .create_invoice(&NewInvoice {
                customer_id: buyer.id,
                date: None,
                payment_type: "card".into(),
                lines: vec![NewInvoiceLine {
                    item_id: pipe.id,
                    qty: 4.0,
                    rate: None,
                    tax_rate: None,
                }],
            })
            .unwrap();
        (invoice.id, invoice.invoice_no)
    };

    // Fresh handle on the same file: migrations re-run as a no-op, data is all there.
    let db = Db::open(&path).unwrap();
    let reopened = db.get_invoice(invoice_id).unwrap().expect("invoice persisted");
    assert_eq!(reopened.invoice_no, invoice_no);
    assert_eq!(reopened.subtotal, 1_142.00);
    assert_eq!(db.invoice_lines(invoice_id).unwrap().len(), 1);
    assert_eq!(db.list_todays_invoices().unwrap().len(), 1);
    assert_eq!(db.pending_sync_rows_for("invoices").unwrap()[0].row_id, invoice_id);
}

#[test]
fn adding_a_line_retotals_the_invoice_and_queues_an_update() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let steel = item(&db, "TMT-12MM");

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "cash".into(),
            lines: vec![NewInvoiceLine { item_id: steel.id, qty: 1.0, rate: None, tax_rate: None }],
        })
        .unwrap();
    assert_eq!(invoice.subtotal, 620.00);

    let line = db
        .add_line_item(
            invoice.id,
            &NewInvoiceLine { item_id: steel.id, qty: 2.0, rate: None, tax_rate: None },
        )
        .unwrap();
    assert_eq!(line.line_total, 1_240.00);

    let updated = db.get_invoice(invoice.id).unwrap().unwrap();
    assert_eq!(updated.subtotal, 1_860.00);
    assert_eq!(updated.cgst, 167.40);
    assert_eq!(updated.sgst, 167.40);
    assert_eq!(updated.grand_total, 2_194.80);
    assert_eq!(db.invoice_lines(invoice.id).unwrap().len(), 2);

    let ops: Vec<String> =
        db.pending_sync_rows_for("invoices").unwrap().into_iter().map(|r| r.op).collect();
    assert_eq!(ops, vec!["insert", "update"]);
}

#[test]
fn an_inter_state_customer_is_billed_igst() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9845567890"); // Deccan Supplies, KA
    let steel = item(&db, "TMT-12MM");

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "credit".into(),
            lines: vec![NewInvoiceLine {
                item_id: steel.id,
                qty: 10.0,
                rate: None,
                tax_rate: None,
            }],
        })
        .unwrap();

    assert_eq!(invoice.subtotal, 6_200.00);
    assert_eq!(invoice.cgst, 0.0);
    assert_eq!(invoice.sgst, 0.0);
    assert_eq!(invoice.igst, 1_116.00);
    assert_eq!(invoice.grand_total, 7_316.00);
}

#[test]
fn invoice_numbers_run_in_sequence_within_a_financial_year() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let steel = item(&db, "TMT-12MM");

    let mut raise = |date: &str| {
        db.create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: Some(date.into()),
            payment_type: "cash".into(),
            lines: vec![NewInvoiceLine { item_id: steel.id, qty: 1.0, rate: None, tax_rate: None }],
        })
        .unwrap()
        .invoice_no
    };

    assert_eq!(raise("2026-09-11"), "RI-2026-0001");
    assert_eq!(raise("2026-09-12"), "RI-2026-0002");
    // Still FY 2026 — the year rolls, the counter does not.
    assert_eq!(raise("2027-03-31"), "RI-2026-0003");
    // 1 April starts FY 2027 and the sequence restarts.
    assert_eq!(raise("2027-04-01"), "RI-2027-0001");
}

#[test]
fn searches_behave_on_misses_and_partial_input() {
    let db = seeded_db();

    assert!(db.search_customer("9999999999").unwrap().is_none());
    assert!(db.search_customer("   ").unwrap().is_none());
    assert_eq!(db.search_customer(" 9840012345 ").unwrap().unwrap().name, "Sri Balaji Traders");

    // Substring, over code and description, case-insensitively.
    assert_eq!(db.search_item("tmt").unwrap().len(), 1);
    assert_eq!(db.search_item("Pipe").unwrap().len(), 1);
    assert!(db.search_item("nothing-like-this").unwrap().is_empty());
    assert_eq!(db.search_item("").unwrap().len(), 5);
}

#[test]
fn bad_references_and_quantities_are_rejected() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let steel = item(&db, "TMT-12MM");

    let missing_customer = db.create_invoice(&NewInvoice {
        customer_id: 9_999,
        date: None,
        payment_type: "cash".into(),
        lines: vec![],
    });
    assert!(matches!(missing_customer, Err(CoreError::NotFound(_))));

    let bad_qty = db.create_invoice(&NewInvoice {
        customer_id: buyer.id,
        date: None,
        payment_type: "cash".into(),
        lines: vec![NewInvoiceLine { item_id: steel.id, qty: 0.0, rate: None, tax_rate: None }],
    });
    assert!(matches!(bad_qty, Err(CoreError::Invalid(_))));

    let bad_date = db.create_invoice(&NewInvoice {
        customer_id: buyer.id,
        date: Some("11-09-2026".into()),
        payment_type: "cash".into(),
        lines: vec![],
    });
    assert!(matches!(bad_date, Err(CoreError::Invalid(_))));

    // A rejected invoice leaves nothing behind, queue included.
    assert!(db.pending_sync_rows_for("invoices").unwrap().is_empty());
    assert!(db.pending_sync_rows_for("invoice_lines").unwrap().is_empty());
}

#[test]
fn seeding_twice_does_not_duplicate_rows() {
    let mut db = seeded_db();
    seed::seed_demo_data(&mut db).unwrap();

    assert_eq!(db.search_item("").unwrap().len(), 5);
    assert_eq!(db.search_customer("9840012345").unwrap().unwrap().name, "Sri Balaji Traders");
    // Second pass queues updates rather than a second set of inserts.
    let ops: Vec<String> =
        db.pending_sync_rows_for("items").unwrap().into_iter().map(|r| r.op).collect();
    assert_eq!(ops.iter().filter(|o| *o == "insert").count(), 5);
    assert_eq!(ops.iter().filter(|o| *o == "update").count(), 5);
}
