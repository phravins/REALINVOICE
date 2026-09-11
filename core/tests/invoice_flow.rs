//! End-to-end tests: an invoice goes in through `Db`, and comes back out of a freshly
//! reopened SQLite file with its lines and its `sync_queue` rows intact.

use realinvoice_core::{
    seed, CoreError, Db, InvoiceFilter, NewCustomer, NewInvoice, NewInvoiceLine, NewItem,
};

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
    assert_eq!(db.search_item("").unwrap().len(), seed::demo_items().len());
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

    assert_eq!(db.search_item("").unwrap().len(), seed::demo_items().len());
    assert_eq!(db.search_customer("9840012345").unwrap().unwrap().name, "Sri Balaji Traders");
    // Second pass queues updates rather than a second set of inserts.
    let seeded = seed::demo_items().len();
    let ops: Vec<String> =
        db.pending_sync_rows_for("items").unwrap().into_iter().map(|r| r.op).collect();
    assert_eq!(ops.iter().filter(|o| *o == "insert").count(), seeded);
    assert_eq!(ops.iter().filter(|o| *o == "update").count(), seeded);
}

#[test]
fn a_new_customer_can_be_registered_from_the_counter() {
    let mut db = seeded_db();

    let created = db
        .create_customer(&NewCustomer {
            name: "  Anand Electricals  ".into(),
            gstin: Some("  33AAFCA1234M1Z9  ".into()),
            place_of_supply: "tn".into(),
            mobile: " 9884455661 ".into(),
        })
        .expect("register customer");

    // Input is trimmed and the state code normalised, so GST comparison stays reliable.
    assert_eq!(created.name, "Anand Electricals");
    assert_eq!(created.gstin.as_deref(), Some("33AAFCA1234M1Z9"));
    assert_eq!(created.place_of_supply, "TN");
    assert_eq!(created.mobile, "9884455661");

    // Immediately findable by the same search the counter just missed on.
    assert_eq!(db.search_customer("9884455661").unwrap().unwrap(), created);

    let queued = db.pending_sync_rows_for("customers").unwrap();
    let mine = queued.iter().find(|r| r.row_id == created.id).expect("queued");
    assert_eq!(mine.op, "insert");
    assert!(mine.synced_at.is_none());
}

#[test]
fn registering_a_duplicate_mobile_is_refused() {
    let mut db = seeded_db();

    let duplicate = db.create_customer(&NewCustomer {
        name: "Someone Else".into(),
        gstin: None,
        place_of_supply: "TN".into(),
        mobile: "9840012345".into(),
    });
    assert!(matches!(duplicate, Err(CoreError::Invalid(_))));

    // The existing record is untouched.
    assert_eq!(db.search_customer("9840012345").unwrap().unwrap().name, "Sri Balaji Traders");
}

#[test]
fn a_blank_gstin_is_stored_as_unregistered() {
    let mut db = seeded_db();

    let created = db
        .create_customer(&NewCustomer {
            name: "Cash Counter Buyer".into(),
            gstin: Some("   ".into()),
            place_of_supply: "TN".into(),
            mobile: "9112233445".into(),
        })
        .unwrap();

    assert_eq!(created.gstin, None, "whitespace is not a GSTIN");
}

#[test]
fn incomplete_customers_are_refused() {
    let mut db = seeded_db();
    let base = NewCustomer {
        name: "Valid Name".into(),
        gstin: None,
        place_of_supply: "TN".into(),
        mobile: "9111111111".into(),
    };

    let no_name = db.create_customer(&NewCustomer { name: "  ".into(), ..base.clone() });
    assert!(matches!(no_name, Err(CoreError::Invalid(_))));

    let no_mobile = db.create_customer(&NewCustomer { mobile: "".into(), ..base.clone() });
    assert!(matches!(no_mobile, Err(CoreError::Invalid(_))));

    let no_state = db.create_customer(&NewCustomer { place_of_supply: " ".into(), ..base });
    assert!(matches!(no_state, Err(CoreError::Invalid(_))));

    // Nothing half-written landed.
    assert!(db.search_customer("9111111111").unwrap().is_none());
}

/// The stage-2 worked example, billed through the seeded catalogue: a 42U rack and five
/// enterprise licences to an intra-state Tamil Nadu buyer.
#[test]
fn the_worked_example_totals_to_one_lakh_twentythree_thousand_nine_hundred() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9600011223"); // Ishta Capital Investments, TN
    let rack = item(&db, "RACK-42U-PRO");
    let license = item(&db, "ABCOS-ENT-LIC");

    assert_eq!(rack.rate, 45_000.0);
    assert_eq!(rack.description, "42U Server Rack Pro");
    assert_eq!(license.rate, 12_000.0);
    assert_eq!(license.description, "aBCOS Enterprise Lic");

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "credit".into(),
            lines: vec![
                NewInvoiceLine { item_id: rack.id, qty: 1.0, rate: None, tax_rate: None },
                NewInvoiceLine { item_id: license.id, qty: 5.0, rate: None, tax_rate: None },
            ],
        })
        .unwrap();

    assert_eq!(invoice.subtotal, 105_000.00);
    assert_eq!(invoice.cgst, 9_450.00);
    assert_eq!(invoice.sgst, 9_450.00);
    assert_eq!(invoice.igst, 0.0);
    assert_eq!(invoice.grand_total, 123_900.00);
}

/// Invoice numbers are allocated inside the write transaction, so two connections
/// billing at the same moment cannot be handed the same number. This is the guard for
/// the multi-console setup a later stage brings.
#[test]
fn concurrent_saves_never_collide_on_an_invoice_number() {
    use std::sync::{Arc, Barrier};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite");

    let (customer_id, item_id) = {
        let mut db = Db::open(&path).unwrap();
        seed::seed_demo_data(&mut db).unwrap();
        (customer(&db, "9840012345").id, item(&db, "TMT-12MM").id)
    };

    const WRITERS: usize = 8;
    // Every thread opens its own connection and they all start together, so the saves
    // genuinely overlap rather than queueing behind each other by accident.
    let barrier = Arc::new(Barrier::new(WRITERS));
    let mut handles = Vec::new();

    for _ in 0..WRITERS {
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let mut db = Db::open(&path).unwrap();
            barrier.wait();
            db.create_invoice(&NewInvoice {
                customer_id,
                date: Some("2026-09-11".into()),
                payment_type: "cash".into(),
                lines: vec![NewInvoiceLine { item_id, qty: 1.0, rate: None, tax_rate: None }],
            })
            .expect("save under contention")
            .invoice_no
        }));
    }

    let mut numbers: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    numbers.sort();
    numbers.dedup();
    assert_eq!(numbers.len(), WRITERS, "every save got its own number");

    // And they form one unbroken run, not a sparse set with gaps.
    let expected: Vec<String> = (1..=WRITERS).map(|n| format!("RI-2026-{n:04}")).collect();
    assert_eq!(numbers, expected);

    // Each save queued its own invoice row for sync.
    let db = Db::open(&path).unwrap();
    assert_eq!(db.pending_sync_rows_for("invoices").unwrap().len(), WRITERS);
    assert_eq!(db.list_invoices_for_date("2026-09-11".parse().unwrap()).unwrap().len(), WRITERS);
}

/// Raises invoices across several dates, customers and totals so the history query has
/// something to narrow. Returns them oldest first.
fn history_fixture(db: &mut Db) -> Vec<realinvoice_core::Invoice> {
    let ishta = customer(db, "9600011223").id; // TN
    let kaveri = customer(db, "9791045678").id; // TN
    let deccan = customer(db, "9845567890").id; // KA
    let rack = item(db, "RACK-42U-PRO").id;
    let license = item(db, "ABCOS-ENT-LIC").id;
    let cement = item(db, "CEM-OPC-53").id;

    let mut raise = |customer_id: i64, date: &str, pay: &str, lines: Vec<NewInvoiceLine>| {
        db.create_invoice(&NewInvoice {
            customer_id,
            date: Some(date.into()),
            payment_type: pay.into(),
            lines,
        })
        .unwrap()
    };

    vec![
        raise(
            kaveri,
            "2026-08-20",
            "upi",
            vec![NewInvoiceLine { item_id: cement, qty: 20.0, rate: None, tax_rate: None }],
        ),
        raise(
            deccan,
            "2026-09-09",
            "credit",
            vec![NewInvoiceLine { item_id: license, qty: 2.0, rate: None, tax_rate: None }],
        ),
        raise(
            ishta,
            "2026-09-11",
            "cash",
            vec![
                NewInvoiceLine { item_id: rack, qty: 1.0, rate: None, tax_rate: None },
                NewInvoiceLine { item_id: license, qty: 5.0, rate: None, tax_rate: None },
            ],
        ),
        raise(
            kaveri,
            "2026-09-11",
            "card",
            vec![NewInvoiceLine { item_id: rack, qty: 2.0, rate: None, tax_rate: None }],
        ),
    ]
}

#[test]
fn history_lists_everything_newest_first() {
    let mut db = seeded_db();
    history_fixture(&mut db);

    let all = db.list_invoices(&InvoiceFilter::default()).unwrap();
    assert_eq!(all.len(), 4);

    let numbers: Vec<&str> = all.iter().map(|s| s.invoice.invoice_no.as_str()).collect();
    // Newest date first; within a date, the later invoice first.
    assert_eq!(numbers, ["RI-2026-0004", "RI-2026-0003", "RI-2026-0002", "RI-2026-0001"]);

    // Each row carries what the list column needs without a second lookup.
    let top = &all[0];
    assert_eq!(top.customer_name, "Kaveri Hardware");
    assert_eq!(top.customer_mobile, "9791045678");
    assert_eq!(top.line_count, 1);
    assert_eq!(top.invoice.payment_type, "card");
    assert_eq!(top.invoice.grand_total, 106_200.00);
    assert!(!top.invoice.created_at.is_empty());
}

#[test]
fn history_narrows_by_date_range() {
    let mut db = seeded_db();
    history_fixture(&mut db);

    let today = db
        .list_invoices(&InvoiceFilter {
            from: Some("2026-09-11".into()),
            to: Some("2026-09-11".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(today.len(), 2);
    assert!(today.iter().all(|s| s.invoice.date == "2026-09-11"));

    // A week that takes in the 9th but not August.
    let week = db
        .list_invoices(&InvoiceFilter {
            from: Some("2026-09-07".into()),
            to: Some("2026-09-13".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(week.len(), 3);

    // A custom range that excludes every invoice comes back empty, not erroring.
    let none = db
        .list_invoices(&InvoiceFilter {
            from: Some("2026-01-01".into()),
            to: Some("2026-01-31".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(none.is_empty());

    // An open-ended bound filters on one side only.
    let since_september = db
        .list_invoices(&InvoiceFilter { from: Some("2026-09-01".into()), ..Default::default() })
        .unwrap();
    assert_eq!(since_september.len(), 3);
}

#[test]
fn history_text_filter_matches_customer_or_invoice_number() {
    let mut db = seeded_db();
    history_fixture(&mut db);

    let by_customer = db
        .list_invoices(&InvoiceFilter { text: Some("kaveri".into()), ..Default::default() })
        .unwrap();
    assert_eq!(by_customer.len(), 2, "case-insensitive on customer name");

    let by_number = db
        .list_invoices(&InvoiceFilter { text: Some("RI-2026-0003".into()), ..Default::default() })
        .unwrap();
    assert_eq!(by_number.len(), 1);
    assert_eq!(by_number[0].customer_name, "Ishta Capital Investments");

    // Partial numbers work too, since the match is a substring.
    assert_eq!(
        db.list_invoices(&InvoiceFilter { text: Some("0004".into()), ..Default::default() })
            .unwrap()
            .len(),
        1
    );

    // Text and dates compose.
    let both = db
        .list_invoices(&InvoiceFilter {
            from: Some("2026-09-11".into()),
            to: Some("2026-09-11".into()),
            text: Some("kaveri".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(both.len(), 1);
    assert_eq!(both[0].invoice.invoice_no, "RI-2026-0004");

    assert!(db
        .list_invoices(&InvoiceFilter { text: Some("nobody".into()), ..Default::default() })
        .unwrap()
        .is_empty());

    // Whitespace is not a filter.
    assert_eq!(
        db.list_invoices(&InvoiceFilter { text: Some("   ".into()), ..Default::default() })
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn invoice_detail_returns_exactly_what_was_saved() {
    let mut db = seeded_db();
    let raised = history_fixture(&mut db);
    let worked_example = &raised[2]; // 1 rack + 5 licences to Ishta, intra-state TN

    let detail = db.get_invoice_detail(worked_example.id).unwrap().expect("detail");

    assert_eq!(&detail.invoice, worked_example);
    assert_eq!(detail.invoice.subtotal, 105_000.00);
    assert_eq!(detail.invoice.cgst, 9_450.00);
    assert_eq!(detail.invoice.sgst, 9_450.00);
    assert_eq!(detail.invoice.igst, 0.0);
    assert_eq!(detail.invoice.grand_total, 123_900.00);

    assert_eq!(detail.customer.name, "Ishta Capital Investments");
    assert_eq!(detail.customer.gstin.as_deref(), Some("33AAAAA0000A1Z1"));

    assert_eq!(detail.lines.len(), 2);
    assert_eq!(detail.lines[0].item_code, "RACK-42U-PRO");
    assert_eq!(detail.lines[0].description, "42U Server Rack Pro");
    assert_eq!(detail.lines[0].uom, "NOS");
    assert_eq!(detail.lines[0].line.qty, 1.0);
    assert_eq!(detail.lines[0].line.line_total, 45_000.00);
    assert_eq!(detail.lines[1].item_code, "ABCOS-ENT-LIC");
    assert_eq!(detail.lines[1].line.qty, 5.0);
    assert_eq!(detail.lines[1].line.line_total, 60_000.00);

    // The lines add up to the stored subtotal — what a reprint puts on paper.
    let summed: f64 = detail.lines.iter().map(|l| l.line.line_total).sum();
    assert_eq!(summed, detail.invoice.subtotal);
}

#[test]
fn detail_of_an_inter_state_invoice_carries_igst() {
    let mut db = seeded_db();
    let raised = history_fixture(&mut db);

    let detail = db.get_invoice_detail(raised[1].id).unwrap().unwrap();
    assert_eq!(detail.customer.place_of_supply, "KA");
    assert_eq!(detail.invoice.cgst, 0.0);
    assert_eq!(detail.invoice.sgst, 0.0);
    assert_eq!(detail.invoice.igst, 4_320.00);
    assert_eq!(detail.invoice.grand_total, 28_320.00);
}

#[test]
fn detail_of_an_unknown_invoice_is_none() {
    let db = seeded_db();
    assert!(db.get_invoice_detail(9_999).unwrap().is_none());
}

#[test]
fn history_respects_a_limit() {
    let mut db = seeded_db();
    history_fixture(&mut db);

    let capped = db.list_invoices(&InvoiceFilter { limit: Some(2), ..Default::default() }).unwrap();
    assert_eq!(capped.len(), 2);
    // Still the newest two, not an arbitrary pair.
    assert_eq!(capped[0].invoice.invoice_no, "RI-2026-0004");
    assert_eq!(capped[1].invoice.invoice_no, "RI-2026-0003");
}
