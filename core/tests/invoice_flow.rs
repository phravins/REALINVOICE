//! End-to-end tests: an invoice goes in through `Db`, and comes back out of a freshly
//! reopened SQLite file with its lines and its `sync_queue` rows intact.

use realinvoice_core::{
    seed, CoreError, DateRange, Db, InvoiceFilter, ItemFilter, NewCustomer, NewInvoice,
    NewInvoiceLine, NewItem, NewUser, Role, SyncBatch, User,
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
            created_by_user_id: None,
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
            created_by_user_id: None,
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
                created_by_user_id: None,
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
            created_by_user_id: None,
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
            created_by_user_id: None,
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
            created_by_user_id: None,
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
        created_by_user_id: None,
        lines: vec![],
    });
    assert!(matches!(missing_customer, Err(CoreError::NotFound(_))));

    let bad_qty = db.create_invoice(&NewInvoice {
        customer_id: buyer.id,
        date: None,
        payment_type: "cash".into(),
        created_by_user_id: None,
        lines: vec![NewInvoiceLine { item_id: steel.id, qty: 0.0, rate: None, tax_rate: None }],
    });
    assert!(matches!(bad_qty, Err(CoreError::Invalid(_))));

    let bad_date = db.create_invoice(&NewInvoice {
        customer_id: buyer.id,
        date: Some("11-09-2026".into()),
        payment_type: "cash".into(),
        created_by_user_id: None,
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
            created_by_user_id: None,
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
                created_by_user_id: None,
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
            created_by_user_id: None,
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

// ------------------------------------------------------------------ sign-in

/// Creates the owner the way the setup screen does: a person types these in, and this is
/// all that happens. There is no other route into an empty users table.
fn owner(db: &mut Db) -> User {
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
fn a_fresh_installation_has_no_accounts_at_all() {
    // What the app keys the setup screen off. Nothing is seeded, generated or printed:
    // until somebody creates an account, there is nothing on this machine to sign in as.
    let db = Db::open_in_memory().unwrap();
    assert_eq!(db.count_users().unwrap(), 0);
    assert!(db.list_users().unwrap().is_empty());
    assert!(db.find_user("admin").unwrap().is_none(), "no default account exists");
    assert!(db.verify_login("admin", "admin").unwrap().is_none());
}

#[test]
fn the_account_created_at_setup_can_sign_in() {
    let mut db = Db::open_in_memory().unwrap();
    let created = owner(&mut db);
    assert_eq!(created.role, Role::Owner);
    assert_eq!(db.count_users().unwrap(), 1, "the app is now set up");

    let signed_in = db.verify_login("priya", "counter-top-2026").unwrap().expect("sign-in");
    assert_eq!(signed_in, created);

    // Case-insensitive username, exact password.
    assert!(db.verify_login("PRIYA", "counter-top-2026").unwrap().is_some());
    assert!(db.verify_login("priya", "COUNTER-TOP-2026").unwrap().is_none());
}

#[test]
fn a_wrong_password_or_unknown_user_both_return_none() {
    let mut db = Db::open_in_memory().unwrap();
    owner(&mut db);

    assert!(db.verify_login("priya", "not the password").unwrap().is_none());
    assert!(db.verify_login("nobody", "whatever").unwrap().is_none());
    assert!(db.verify_login("", "").unwrap().is_none());
}

#[test]
fn the_password_is_never_stored_or_returned_in_the_clear() {
    let mut db = Db::open_in_memory().unwrap();
    let created = owner(&mut db);
    let password = "counter-top-2026";

    // The User that crosses into the frontend carries no hash and no password.
    let serialized = serde_json::to_string(&created).unwrap();
    assert!(!serialized.contains(password));
    assert!(!serialized.contains("password"), "{serialized}");

    // And nothing about an account is queued for sync — logins are per-machine.
    let stored: String =
        db.pending_sync_rows().unwrap().iter().map(|r| r.payload_json.clone()).collect();
    assert!(!stored.contains(password), "users must not be queued for sync");
    assert!(!stored.contains("priya"), "users must not be queued for sync");
}

#[test]
fn an_owner_can_add_staff_and_duplicates_are_refused() {
    let mut db = Db::open_in_memory().unwrap();
    owner(&mut db);

    let cashier = db
        .create_user(
            &NewUser {
                username: "  Meena  ".into(),
                display_name: "Meena R".into(),
                role: Role::Cashier,
            },
            "counter-password",
        )
        .unwrap();
    assert_eq!(cashier.username, "meena", "usernames normalise to lowercase");
    assert_eq!(cashier.role, Role::Cashier);
    assert!(db.verify_login("MEENA", "counter-password").unwrap().is_some());

    let duplicate = db.create_user(
        &NewUser {
            username: "meena".into(),
            display_name: "Someone Else".into(),
            role: Role::Cashier,
        },
        "another-password",
    );
    assert!(matches!(duplicate, Err(CoreError::Invalid(_))));

    let too_short = db.create_user(
        &NewUser { username: "raj".into(), display_name: "Raj".into(), role: Role::Cashier },
        "short",
    );
    assert!(matches!(too_short, Err(CoreError::Invalid(_))));
    assert!(db.find_user("raj").unwrap().is_none(), "nothing half-created");
}

#[test]
fn the_users_screen_lists_every_account_oldest_first() {
    let mut db = Db::open_in_memory().unwrap();
    owner(&mut db);
    db.create_user(
        &NewUser { username: "meena".into(), display_name: "Meena R".into(), role: Role::Cashier },
        "counter-password",
    )
    .unwrap();

    let listed = db.list_users().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].username, "priya", "the account that set the machine up is first");
    assert_eq!(listed[0].role, Role::Owner);
    assert_eq!(listed[1].username, "meena");
    assert_eq!(listed[1].role, Role::Cashier);

    // Nothing in the list carries a credential.
    let serialized = serde_json::to_string(&listed).unwrap();
    assert!(!serialized.contains("counter-password"));
    assert!(!serialized.contains("$2"), "no bcrypt hash reaches the screen");
}

#[test]
fn an_invoice_records_who_billed_it() {
    let mut db = seeded_db();
    let biller = owner(&mut db);
    let buyer = customer(&db, "9600011223");
    let rack = item(&db, "RACK-42U-PRO");

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "upi".into(),
            created_by_user_id: Some(biller.id),
            lines: vec![NewInvoiceLine { item_id: rack.id, qty: 1.0, rate: None, tax_rate: None }],
        })
        .unwrap();

    assert_eq!(invoice.created_by_user_id, Some(biller.id));

    // The history list and the detail view both name the biller.
    let listed = db.list_invoices(&InvoiceFilter::default()).unwrap();
    assert_eq!(listed[0].created_by.as_deref(), Some("Priya Raman"));

    let detail = db.get_invoice_detail(invoice.id).unwrap().unwrap();
    assert_eq!(detail.created_by.unwrap().display_name, "Priya Raman");

    // It survives a reopen, and the queued payload carries it for the sync worker.
    let queued = &db.pending_sync_rows_for("invoices").unwrap()[0];
    let payload: serde_json::Value = serde_json::from_str(&queued.payload_json).unwrap();
    assert_eq!(payload["created_by_user_id"], biller.id);
}

#[test]
fn an_unattributed_invoice_is_still_valid() {
    // Invoices raised before sign-in existed have nobody to attribute them to.
    let mut db = seeded_db();
    let buyer = customer(&db, "9600011223");
    let rack = item(&db, "RACK-42U-PRO");

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            lines: vec![NewInvoiceLine { item_id: rack.id, qty: 1.0, rate: None, tax_rate: None }],
        })
        .unwrap();

    assert_eq!(invoice.created_by_user_id, None);
    assert_eq!(db.list_invoices(&InvoiceFilter::default()).unwrap()[0].created_by, None);
    assert!(db.get_invoice_detail(invoice.id).unwrap().unwrap().created_by.is_none());
}

#[test]
fn payment_type_is_stored_as_chosen() {
    let mut db = seeded_db();
    let biller = owner(&mut db);
    let buyer = customer(&db, "9600011223");
    let rack = item(&db, "RACK-42U-PRO");

    for chosen in ["upi", "cash", "card"] {
        let invoice = db
            .create_invoice(&NewInvoice {
                customer_id: buyer.id,
                date: None,
                payment_type: chosen.into(),
                created_by_user_id: Some(biller.id),
                lines: vec![NewInvoiceLine {
                    item_id: rack.id,
                    qty: 1.0,
                    rate: None,
                    tax_rate: None,
                }],
            })
            .unwrap();
        assert_eq!(invoice.payment_type, chosen);
        assert_eq!(db.get_invoice(invoice.id).unwrap().unwrap().payment_type, chosen);
    }
}

// ----------------------------------------------------------------- settings

#[test]
fn a_local_preference_round_trips_and_overwrites() {
    let mut db = Db::open_in_memory().unwrap();

    assert_eq!(db.get_setting("ui.theme").unwrap(), None, "unset until chosen");

    db.set_setting("ui.theme", "light").unwrap();
    assert_eq!(db.get_setting("ui.theme").unwrap().as_deref(), Some("light"));

    // Choosing again replaces rather than accumulating rows.
    db.set_setting("ui.theme", "dark").unwrap();
    assert_eq!(db.get_setting("ui.theme").unwrap().as_deref(), Some("dark"));

    assert!(db.set_setting("  ", "x").is_err());
    assert_eq!(db.get_setting("never.set").unwrap(), None);
}

#[test]
fn a_preference_survives_reopening_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite");

    {
        let mut db = Db::open(&path).unwrap();
        db.set_setting("ui.theme", "light").unwrap();
    }

    let db = Db::open(&path).unwrap();
    assert_eq!(db.get_setting("ui.theme").unwrap().as_deref(), Some("light"));
}

#[test]
fn preferences_are_not_queued_for_sync() {
    // One till's display preference is not something the back office should receive.
    let mut db = seeded_db();
    let before = db.pending_sync_rows().unwrap().len();

    db.set_setting("ui.theme", "dark").unwrap();

    assert_eq!(db.pending_sync_rows().unwrap().len(), before);
    assert!(db.pending_sync_rows_for("settings").unwrap().is_empty());
}

// -------------------------------------------------------------- inventory

#[test]
fn the_catalogue_lists_and_filters() {
    let db = seeded_db();

    let all = db.list_items(&ItemFilter::default()).unwrap();
    assert_eq!(all.len() as i64, db.count_items().unwrap());
    assert_eq!(all.len(), 7, "the seven demo items");
    // Sorted by code, so the list does not reshuffle between visits.
    let mut sorted = all.iter().map(|i| i.item_code.clone()).collect::<Vec<_>>();
    sorted.sort();
    assert_eq!(sorted, all.iter().map(|i| i.item_code.clone()).collect::<Vec<_>>());

    // Matches a code or a description, case-insensitively.
    let by_code =
        db.list_items(&ItemFilter { text: Some("rack".into()), ..Default::default() }).unwrap();
    assert_eq!(by_code.len(), 1);
    assert_eq!(by_code[0].item_code, "RACK-42U-PRO");

    let by_text =
        db.list_items(&ItemFilter { text: Some("server".into()), ..Default::default() }).unwrap();
    assert!(by_text.iter().any(|i| i.item_code == "RACK-42U-PRO"));

    assert!(db
        .list_items(&ItemFilter { text: Some("nothing-like-this".into()), ..Default::default() })
        .unwrap()
        .is_empty());
}

#[test]
fn adding_an_item_puts_it_in_the_catalogue_and_the_sync_queue() {
    let mut db = seeded_db();
    let before = db.count_items().unwrap();

    let added = db
        .upsert_item(&NewItem {
            item_code: "PATCH-CAT6".into(),
            description: "Cat6 patch cable 2m".into(),
            rate: 180.0,
            tax_rate: 18.0,
            uom: "NOS".into(),
        })
        .unwrap();

    assert_eq!(db.count_items().unwrap(), before + 1);
    assert!(db
        .list_items(&ItemFilter { text: Some("PATCH".into()), ..Default::default() })
        .unwrap()
        .iter()
        .any(|i| i.id == added.id));

    // Unlike users, the catalogue is shared business data and is queued for the
    // back office.
    let queued = db.pending_sync_rows_for("items").unwrap();
    assert!(queued.iter().any(|r| r.row_id == added.id));
}

// -------------------------------------------------------------- analytics

/// Two invoices on one day for one buyer, plus one for another, so every aggregate has
/// more than a single row to fold.
fn billed_days(db: &mut Db) {
    let balaji = customer(db, "9840012345").id;
    let kaveri = customer(db, "9600011223").id;
    let rack = item(db, "RACK-42U-PRO").id;
    let lic = item(db, "ABCOS-ENT-LIC").id;

    let mut raise = |customer_id: i64, date: &str, pay: &str, item_id: i64, qty: f64| {
        db.create_invoice(&NewInvoice {
            customer_id,
            date: Some(date.to_string()),
            payment_type: pay.into(),
            created_by_user_id: None,
            lines: vec![NewInvoiceLine { item_id, qty, rate: None, tax_rate: None }],
        })
        .unwrap()
    };

    raise(balaji, "2026-04-01", "cash", rack, 1.0); // 45,000 + 18%
    raise(kaveri, "2026-04-01", "upi", lic, 2.0); // 24,000 + 18%
    raise(balaji, "2026-04-03", "card", rack, 1.0); // 45,000 + 18%
}

#[test]
fn the_sales_summary_adds_up_what_was_billed() {
    let mut db = seeded_db();
    billed_days(&mut db);

    let all = db.sales_summary(&DateRange::default()).unwrap();
    assert_eq!(all.invoice_count, 3);
    assert_eq!(all.subtotal, 114_000.0, "45,000 + 24,000 + 45,000");
    // Every buyer is in TN, so it is all CGST + SGST and never IGST.
    assert_eq!(all.cgst, 10_260.0, "9% of 114,000");
    assert_eq!(all.sgst, 10_260.0);
    assert_eq!(all.igst, 0.0);
    assert_eq!(all.tax_total, 20_520.0, "what has to be remitted");
    assert_eq!(all.grand_total, 134_520.0);
    // The parts reconcile with the whole, which is the point of showing them together.
    assert_eq!(all.subtotal + all.tax_total, all.grand_total);
}

#[test]
fn a_date_range_narrows_every_aggregate() {
    let mut db = seeded_db();
    billed_days(&mut db);

    let first_day = DateRange { from: Some("2026-04-01".into()), to: Some("2026-04-01".into()) };
    let day = db.sales_summary(&first_day).unwrap();
    assert_eq!(day.invoice_count, 2);
    assert_eq!(day.subtotal, 69_000.0, "45,000 + 24,000");
    assert_eq!(day.grand_total, 81_420.0);

    // A range with nothing in it is zero, not an error and not the unfiltered total.
    let quiet = DateRange { from: Some("2026-05-01".into()), to: Some("2026-05-31".into()) };
    let none = db.sales_summary(&quiet).unwrap();
    assert_eq!(none.invoice_count, 0);
    assert_eq!(none.grand_total, 0.0);
    assert!(db.daily_totals(&quiet).unwrap().is_empty());
    assert!(db.top_items(&quiet, 5).unwrap().is_empty());
    assert!(db.payment_mix(&quiet).unwrap().is_empty());
}

#[test]
fn daily_totals_have_one_row_per_billed_day_in_order() {
    let mut db = seeded_db();
    billed_days(&mut db);

    let days = db.daily_totals(&DateRange::default()).unwrap();
    assert_eq!(days.len(), 2, "the quiet day between is absent, not zero");
    assert_eq!(days[0].date, "2026-04-01", "oldest first");
    assert_eq!(days[0].invoice_count, 2);
    assert_eq!(days[0].cgst_sgst, 12_420.0, "18% of 69,000");
    assert_eq!(days[0].igst, 0.0);
    assert_eq!(days[1].date, "2026-04-03");
    assert_eq!(days[1].grand_total, 53_100.0);

    // The days add back up to the summary over the same range.
    let summed: f64 = days.iter().map(|d| d.grand_total).sum();
    assert_eq!(summed, db.sales_summary(&DateRange::default()).unwrap().grand_total);
}

#[test]
fn top_items_ranks_by_revenue_not_by_quantity() {
    let mut db = seeded_db();
    billed_days(&mut db);

    let top = db.top_items(&DateRange::default(), 5).unwrap();
    assert_eq!(top.len(), 2);
    // The licence sold twice as many units; the rack still earned more.
    assert_eq!(top[0].item_code, "RACK-42U-PRO");
    assert_eq!(top[0].qty, 2.0);
    assert_eq!(top[0].revenue, 90_000.0);
    assert_eq!(top[1].item_code, "ABCOS-ENT-LIC");
    assert_eq!(top[1].qty, 2.0);
    assert_eq!(top[1].revenue, 24_000.0);

    assert_eq!(db.top_items(&DateRange::default(), 1).unwrap().len(), 1, "limit is honoured");
}

#[test]
fn the_payment_mix_covers_every_invoice_once() {
    let mut db = seeded_db();
    billed_days(&mut db);

    let mix = db.payment_mix(&DateRange::default()).unwrap();
    assert_eq!(mix.len(), 3, "cash, upi and card each appear once");

    let counted: i64 = mix.iter().map(|m| m.invoice_count).sum();
    let billed: f64 = mix.iter().map(|m| m.grand_total).sum();
    let all = db.sales_summary(&DateRange::default()).unwrap();
    assert_eq!(counted, all.invoice_count, "no invoice is missed or double counted");
    assert_eq!(billed, all.grand_total);

    // Biggest share first: cash and card both took a rack, UPI took the licences.
    assert_eq!(mix[2].payment_type, "upi");
}

// ------------------------------------------------------------------ sync queue

#[test]
fn a_batch_is_taken_oldest_first_and_capped() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let rack = item(&db, "RACK-42U-PRO");
    db.create_invoice(&NewInvoice {
        customer_id: buyer.id,
        date: None,
        payment_type: "cash".into(),
        created_by_user_id: None,
        lines: vec![NewInvoiceLine { item_id: rack.id, qty: 1.0, rate: None, tax_rate: None }],
    })
    .unwrap();

    let all = db.pending_sync_rows().unwrap();
    assert!(all.len() > 3, "the demo data and the invoice are queued");

    let batch = db.next_sync_batch(3).unwrap();
    assert_eq!(batch.len(), 3, "the cap is honoured");

    // Oldest first, and that is load-bearing: a customer is queued before the invoice
    // that references it, and an invoice before its lines, so sending in this order means
    // the far end never sees a row whose parent has not arrived.
    let ids: Vec<i64> = batch.iter().map(|r| r.id).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
    assert_eq!(ids[0], all[0].id);

    let order: Vec<&str> = all.iter().map(|r| r.table_name.as_str()).collect();
    let invoice_at = order.iter().position(|t| *t == "invoices").unwrap();
    let line_at = order.iter().position(|t| *t == "invoice_lines").unwrap();
    assert!(invoice_at < line_at, "an invoice is queued before its lines");
}

#[test]
fn marking_a_batch_sent_removes_it_from_the_queue_and_nothing_else() {
    let mut db = seeded_db();
    let before = db.pending_sync_count().unwrap();
    assert_eq!(before as usize, db.pending_sync_rows().unwrap().len());

    let batch = db.next_sync_batch(5).unwrap();
    let ids: Vec<i64> = batch.iter().map(|r| r.id).collect();

    assert_eq!(db.mark_synced(&ids).unwrap(), 5);
    assert_eq!(db.pending_sync_count().unwrap(), before - 5);

    // The next batch carries on from where that one stopped, never repeating it.
    let next = db.next_sync_batch(5).unwrap();
    assert!(next.iter().all(|r| !ids.contains(&r.id)));

    // Marking the same rows again is harmless — a duplicate acknowledgement must not
    // rewrite when they were sent, or mark anything else.
    assert_eq!(db.mark_synced(&ids).unwrap(), 0, "already-sent rows are not re-marked");
    assert_eq!(db.pending_sync_count().unwrap(), before - 5);
    assert_eq!(db.mark_synced(&[]).unwrap(), 0);
}

#[test]
fn a_failed_push_loses_nothing() {
    // What the worker does on a non-2xx or a network error: mark nothing. The same batch
    // has to come back, in the same order, or a rejected batch would cost records.
    let mut db = seeded_db();
    let first = db.next_sync_batch(4).unwrap();

    // ... no mark_synced call ...

    let retry = db.next_sync_batch(4).unwrap();
    assert_eq!(
        first.iter().map(|r| r.id).collect::<Vec<_>>(),
        retry.iter().map(|r| r.id).collect::<Vec<_>>(),
        "a failed batch is retried as the same batch"
    );

    // And billing during the outage simply lengthens the queue.
    let buyer = customer(&db, "9840012345");
    let rack = item(&db, "RACK-42U-PRO");
    let before = db.pending_sync_count().unwrap();
    db.create_invoice(&NewInvoice {
        customer_id: buyer.id,
        date: None,
        payment_type: "cash".into(),
        created_by_user_id: None,
        lines: vec![NewInvoiceLine { item_id: rack.id, qty: 1.0, rate: None, tax_rate: None }],
    })
    .unwrap();
    assert!(db.pending_sync_count().unwrap() > before, "an offline till keeps billing");
}

#[test]
fn the_wire_format_carries_each_record_as_an_object() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let rack = item(&db, "RACK-42U-PRO");
    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "upi".into(),
            created_by_user_id: None,
            lines: vec![NewInvoiceLine { item_id: rack.id, qty: 1.0, rate: None, tax_rate: None }],
        })
        .unwrap();

    let queued = db.pending_sync_rows().unwrap();
    let batch = SyncBatch::build("POS-01", &queued).unwrap();

    assert_eq!(batch.node_id, "POS-01");
    assert_eq!(batch.rows.len(), queued.len());
    assert_eq!(batch.ids(), queued.iter().map(|r| r.id).collect::<Vec<_>>());

    let row = batch.rows.iter().find(|r| r.table_name == "invoices").unwrap();
    assert_eq!(row.row_id, invoice.id);
    assert_eq!(row.op, "insert");

    // The payload is a JSON object, not the stored string re-quoted: the far end should
    // receive a record it can decode, not a blob it has to parse a second time.
    let payload = row.payload.as_object().expect("an object, not a string");
    assert_eq!(payload["invoice_no"], invoice.invoice_no.as_str());
    assert_eq!(payload["grand_total"], invoice.grand_total);
    assert_eq!(payload["payment_type"], "upi");
    // snake_case keys, matching the column names, so an Ecto schema can map them directly.
    assert!(payload.contains_key("customer_id"));
    assert!(payload.contains_key("created_at"));

    // The whole envelope round-trips, which is what the endpoint will actually receive.
    let json = serde_json::to_string(&batch).unwrap();
    let parsed: SyncBatch = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, batch);
}
