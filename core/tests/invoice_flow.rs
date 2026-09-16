//! End-to-end tests: an invoice goes in through `Db`, and comes back out of a freshly
//! reopened SQLite file with its lines and its `sync_queue` rows intact.

use realinvoice_core::{
    seed, CoreError, CustomerFilter, CustomerSort, DateRange, Db, DiscountType, InvoiceFilter,
    ItemFilter, ItemPrice, LedgerEntryKind, LoginOutcome, NewCreditNote, NewCreditNoteLine,
    NewCustomer, NewInvoice, NewInvoiceLine, NewItem, NewPayment, NewUser, PaymentStatus, Role,
    SyncBatch, User,
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
                NewInvoiceLine {
                    item_id: cement.id,
                    qty: 10.0,
                    rate: None,
                    tax_rate: None,
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                },
                NewInvoiceLine {
                    item_id: steel.id,
                    qty: 5.0,
                    rate: None,
                    tax_rate: None,
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                },
            ],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
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
            price_list_id: None,
            credit_limit: None,
        })
        .unwrap();
    let widget = db
        .upsert_item(&NewItem {
            item_code: "W-1".into(),
            description: "Widget".into(),
            rate: 100.0,
            tax_rate: 18.0,
            uom: "NOS".into(),
            custom: false,
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
                NewInvoiceLine {
                    item_id: widget.id,
                    qty: 2.0,
                    rate: None,
                    tax_rate: None,
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                },
                NewInvoiceLine {
                    item_id: widget.id,
                    qty: 3.0,
                    rate: Some(90.0),
                    tax_rate: None,
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                },
            ],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
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
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                }],
                invoice_discount_type: DiscountType::None,
                invoice_discount_value: 0.0,
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
            lines: vec![NewInvoiceLine {
                item_id: steel.id,
                qty: 1.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
        })
        .unwrap();
    assert_eq!(invoice.subtotal, 620.00);

    let line = db
        .add_line_item(
            invoice.id,
            &NewInvoiceLine {
                item_id: steel.id,
                qty: 2.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            },
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
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
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
            lines: vec![NewInvoiceLine {
                item_id: steel.id,
                qty: 1.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
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
        invoice_discount_type: DiscountType::None,
        invoice_discount_value: 0.0,
    });
    assert!(matches!(missing_customer, Err(CoreError::NotFound(_))));

    let bad_qty = db.create_invoice(&NewInvoice {
        customer_id: buyer.id,
        date: None,
        payment_type: "cash".into(),
        created_by_user_id: None,
        lines: vec![NewInvoiceLine {
            item_id: steel.id,
            qty: 0.0,
            rate: None,
            tax_rate: None,
            discount_type: DiscountType::None,
            discount_value: 0.0,
        }],
        invoice_discount_type: DiscountType::None,
        invoice_discount_value: 0.0,
    });
    assert!(matches!(bad_qty, Err(CoreError::Invalid(_))));

    let bad_date = db.create_invoice(&NewInvoice {
        customer_id: buyer.id,
        date: Some("11-09-2026".into()),
        payment_type: "cash".into(),
        created_by_user_id: None,
        lines: vec![],
        invoice_discount_type: DiscountType::None,
        invoice_discount_value: 0.0,
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
            price_list_id: None,
            credit_limit: None,
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
        price_list_id: None,
        credit_limit: None,
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
            price_list_id: None,
            credit_limit: None,
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
        price_list_id: None,
        credit_limit: None,
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
                NewInvoiceLine {
                    item_id: rack.id,
                    qty: 1.0,
                    rate: None,
                    tax_rate: None,
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                },
                NewInvoiceLine {
                    item_id: license.id,
                    qty: 5.0,
                    rate: None,
                    tax_rate: None,
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                },
            ],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
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
                lines: vec![NewInvoiceLine {
                    item_id,
                    qty: 1.0,
                    rate: None,
                    tax_rate: None,
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                }],
                invoice_discount_type: DiscountType::None,
                invoice_discount_value: 0.0,
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
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
        })
        .unwrap()
    };

    vec![
        raise(
            kaveri,
            "2026-08-20",
            "upi",
            vec![NewInvoiceLine {
                item_id: cement,
                qty: 20.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
        ),
        raise(
            deccan,
            "2026-09-09",
            "credit",
            vec![NewInvoiceLine {
                item_id: license,
                qty: 2.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
        ),
        raise(
            ishta,
            "2026-09-11",
            "cash",
            vec![
                NewInvoiceLine {
                    item_id: rack,
                    qty: 1.0,
                    rate: None,
                    tax_rate: None,
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                },
                NewInvoiceLine {
                    item_id: license,
                    qty: 5.0,
                    rate: None,
                    tax_rate: None,
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                },
            ],
        ),
        raise(
            kaveri,
            "2026-09-11",
            "card",
            vec![NewInvoiceLine {
                item_id: rack,
                qty: 2.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
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
    let mut db = db;
    assert_eq!(db.attempt_login("admin", "admin").unwrap(), LoginOutcome::Invalid);
}

#[test]
fn the_account_created_at_setup_can_sign_in() {
    let mut db = Db::open_in_memory().unwrap();
    let created = owner(&mut db);
    assert_eq!(created.role, Role::Owner);
    assert_eq!(db.count_users().unwrap(), 1, "the app is now set up");

    assert_eq!(
        db.attempt_login("priya", "counter-top-2026").unwrap(),
        LoginOutcome::Ok(created.clone())
    );

    // Case-insensitive username, exact password.
    assert!(matches!(db.attempt_login("PRIYA", "counter-top-2026").unwrap(), LoginOutcome::Ok(_)));
    assert_eq!(db.attempt_login("priya", "COUNTER-TOP-2026").unwrap(), LoginOutcome::Invalid);
}

#[test]
fn a_wrong_password_or_unknown_user_both_return_none() {
    let mut db = Db::open_in_memory().unwrap();
    owner(&mut db);

    assert_eq!(db.attempt_login("priya", "not the password").unwrap(), LoginOutcome::Invalid);
    assert_eq!(db.attempt_login("nobody", "whatever").unwrap(), LoginOutcome::Invalid);
    assert_eq!(db.attempt_login("", "").unwrap(), LoginOutcome::Invalid);
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
    assert!(matches!(db.attempt_login("MEENA", "counter-password").unwrap(), LoginOutcome::Ok(_)));

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
            lines: vec![NewInvoiceLine {
                item_id: rack.id,
                qty: 1.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
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
            lines: vec![NewInvoiceLine {
                item_id: rack.id,
                qty: 1.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
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
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                }],
                invoice_discount_type: DiscountType::None,
                invoice_discount_value: 0.0,
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
            custom: false,
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
            lines: vec![NewInvoiceLine {
                item_id,
                qty,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
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
        lines: vec![NewInvoiceLine {
            item_id: rack.id,
            qty: 1.0,
            rate: None,
            tax_rate: None,
            discount_type: DiscountType::None,
            discount_value: 0.0,
        }],
        invoice_discount_type: DiscountType::None,
        invoice_discount_value: 0.0,
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
        lines: vec![NewInvoiceLine {
            item_id: rack.id,
            qty: 1.0,
            rate: None,
            tax_rate: None,
            discount_type: DiscountType::None,
            discount_value: 0.0,
        }],
        invoice_discount_type: DiscountType::None,
        invoice_discount_value: 0.0,
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
            lines: vec![NewInvoiceLine {
                item_id: rack.id,
                qty: 1.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
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

// ------------------------------------------------------------ login rate limit

/// An owner plus a cashier, so a lockout on one can be shown not to touch the other.
fn two_accounts(db: &mut Db) {
    owner(db);
    db.create_user(
        &NewUser { username: "meena".into(), display_name: "Meena R".into(), role: Role::Cashier },
        "counter-password",
    )
    .unwrap();
}

#[test]
fn five_failures_lock_the_account_and_the_sixth_is_refused_even_when_correct() {
    let mut db = Db::open_in_memory().unwrap();
    two_accounts(&mut db);

    for attempt in 1..=4 {
        assert_eq!(
            db.attempt_login("priya", "wrong").unwrap(),
            LoginOutcome::Invalid,
            "attempt {attempt} should still be allowed through to the password check"
        );
        assert!(db.lockout_for("priya").unwrap().is_none(), "not locked yet at {attempt}");
    }

    // The fifth failure is the one that trips it.
    assert_eq!(db.attempt_login("priya", "wrong").unwrap(), LoginOutcome::Invalid);

    let lockout = db.lockout_for("priya").unwrap().expect("locked out after five");
    assert_eq!(lockout.failures, 5);
    assert_eq!(lockout.remaining, 0);
    assert!(lockout.retry_after_seconds > 0);
    assert!(lockout.retry_after_seconds <= realinvoice_core::LOGIN_WINDOW_MINUTES * 60);

    // The sixth attempt is refused **with the right password**. This is the whole point:
    // a correct credential does not get you past the limiter.
    match db.attempt_login("priya", "counter-top-2026").unwrap() {
        LoginOutcome::LockedOut(l) => assert!(l.retry_after_seconds > 0),
        other => panic!("expected a lockout, got {other:?}"),
    }
}

#[test]
fn one_accounts_lockout_does_not_touch_another() {
    let mut db = Db::open_in_memory().unwrap();
    two_accounts(&mut db);

    for _ in 0..5 {
        db.attempt_login("priya", "wrong").unwrap();
    }
    assert!(db.lockout_for("priya").unwrap().is_some());

    // The cashier is unaffected, and can still sign in normally.
    assert!(db.lockout_for("meena").unwrap().is_none());
    assert!(matches!(db.attempt_login("meena", "counter-password").unwrap(), LoginOutcome::Ok(_)));

    // And a username that does not exist keeps its own count, so one locked account
    // cannot shut the whole machine.
    assert!(db.lockout_for("nobody").unwrap().is_none());
    assert_eq!(db.attempt_login("nobody", "whatever").unwrap(), LoginOutcome::Invalid);
}

#[test]
fn the_limit_counts_one_username_however_it_is_typed() {
    let mut db = Db::open_in_memory().unwrap();
    two_accounts(&mut db);

    // If the limiter keyed on the raw string, these would be five buckets of one rather
    // than one bucket of five, and the limit would be trivially bypassed.
    for spelling in ["priya", "PRIYA", "  priya  ", "Priya", "pRiYa"] {
        assert_eq!(db.attempt_login(spelling, "wrong").unwrap(), LoginOutcome::Invalid);
    }

    assert_eq!(db.recent_failed_logins("priya").unwrap(), 5);
    assert!(db.lockout_for("PRIYA").unwrap().is_some(), "checked under any spelling too");
}

#[test]
fn a_username_that_does_not_exist_locks_out_on_the_same_terms() {
    // Otherwise the lockout itself becomes an oracle: a real account would eventually
    // start answering "locked", and an imaginary one never would.
    let mut db = Db::open_in_memory().unwrap();
    two_accounts(&mut db);

    for _ in 0..5 {
        assert_eq!(db.attempt_login("ghost", "wrong").unwrap(), LoginOutcome::Invalid);
    }

    let lockout = db.lockout_for("ghost").unwrap().expect("an unknown username locks out too");
    assert_eq!(lockout.failures, 5);
    assert!(matches!(db.attempt_login("ghost", "wrong").unwrap(), LoginOutcome::LockedOut(_)));
}

#[test]
fn a_success_does_not_wipe_earlier_failures() {
    let mut db = Db::open_in_memory().unwrap();
    two_accounts(&mut db);

    for _ in 0..4 {
        db.attempt_login("priya", "wrong").unwrap();
    }
    assert_eq!(db.recent_failed_logins("priya").unwrap(), 4);

    // A correct sign-in in the middle is allowed — four is under the limit — but it must
    // not reset the count. If it did, anybody who knew one password could clear the
    // limiter at will and brute-force the rest of the window indefinitely.
    assert!(matches!(db.attempt_login("priya", "counter-top-2026").unwrap(), LoginOutcome::Ok(_)));
    assert_eq!(db.recent_failed_logins("priya").unwrap(), 4, "failures survive a success");

    // So the very next failure is the fifth, and it locks.
    db.attempt_login("priya", "wrong").unwrap();
    assert!(db.lockout_for("priya").unwrap().is_some());
}

#[test]
fn attempting_while_locked_out_does_not_extend_the_lockout() {
    let mut db = Db::open_in_memory().unwrap();
    two_accounts(&mut db);

    for _ in 0..5 {
        db.attempt_login("priya", "wrong").unwrap();
    }
    let first = db.lockout_for("priya").unwrap().unwrap();

    // Hammering a locked account must not keep pushing the deadline out — that would turn
    // the rate limit into a way of keeping the real owner locked out forever.
    for _ in 0..10 {
        assert!(matches!(db.attempt_login("priya", "wrong").unwrap(), LoginOutcome::LockedOut(_)));
    }
    let later = db.lockout_for("priya").unwrap().unwrap();

    assert_eq!(later.failures, 5, "refused attempts are logged but do not count");
    assert!(
        later.retry_after_seconds <= first.retry_after_seconds,
        "the deadline moved closer, never further away"
    );
}

#[test]
fn failures_age_out_of_the_window() {
    let mut db = Db::open_in_memory().unwrap();
    two_accounts(&mut db);

    for _ in 0..5 {
        db.attempt_login("priya", "wrong").unwrap();
    }
    assert!(db.lockout_for("priya").unwrap().is_some());

    // Rather than sleeping fifteen minutes, backdate the recorded attempts past the
    // window — which is exactly what the passage of time does to them.
    db.backdate_login_attempts_for_test("priya", realinvoice_core::LOGIN_WINDOW_MINUTES + 1)
        .unwrap();

    assert_eq!(db.recent_failed_logins("priya").unwrap(), 0, "outside the window");
    assert!(db.lockout_for("priya").unwrap().is_none(), "the lock lifts on its own");
    assert!(matches!(db.attempt_login("priya", "counter-top-2026").unwrap(), LoginOutcome::Ok(_)));

    // The rows are still there — aged out of the window, not deleted.
    assert!(db.login_attempts("priya").unwrap().len() > 5);
}

#[test]
fn every_attempt_is_recorded_and_no_secret_is() {
    let mut db = Db::open_in_memory().unwrap();
    two_accounts(&mut db);

    db.attempt_login("priya", "counter-top-2026").unwrap();
    db.attempt_login("priya", "wrong").unwrap();

    let log = db.login_attempts("priya").unwrap();
    assert_eq!(log.len(), 2, "success and failure alike");
    assert!(log[0].success);
    assert!(!log[1].success);
    assert_eq!(log[0].username, "priya", "stored normalised");
    assert!(log[0].attempted_at.len() >= 19);

    // A failed-login log that recorded what was typed would be a second place passwords
    // leak from — including the near-misses, which are the most valuable kind.
    let serialized = serde_json::to_string(&log).unwrap();
    assert!(!serialized.contains("counter-top-2026"), "{serialized}");
    assert!(!serialized.contains("wrong"), "{serialized}");
    assert!(!serialized.contains("$2"), "no hash either");

    // And none of it is queued for the back office.
    let queued: String =
        db.pending_sync_rows().unwrap().iter().map(|r| r.payload_json.clone()).collect();
    assert!(!queued.contains("login_attempt"));
    assert!(!queued.contains("priya"));
}

// ------------------------------------------------------------------ credit notes

/// Billed: 2 racks at 45,000 (18%) and 40 bags of cement at 410 (28%), to a Tamil Nadu
/// buyer, so intra-state CGST + SGST. Two different tax rates on purpose — a credit note
/// that quietly applied one rate to the whole document would pass a single-rate test.
fn billed_invoice(db: &mut Db) -> realinvoice_core::Invoice {
    let buyer = customer(db, "9840012345");
    let rack = item(db, "RACK-42U-PRO");
    let cement = item(db, "CEM-OPC-53");

    db.create_invoice(&NewInvoice {
        customer_id: buyer.id,
        date: None,
        payment_type: "cash".into(),
        created_by_user_id: None,
        lines: vec![
            NewInvoiceLine {
                item_id: rack.id,
                qty: 2.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            },
            NewInvoiceLine {
                item_id: cement.id,
                qty: 40.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            },
        ],
        invoice_discount_type: DiscountType::None,
        invoice_discount_value: 0.0,
    })
    .unwrap()
}

#[test]
fn a_partial_credit_prices_the_credited_quantity_and_leaves_the_invoice_alone() {
    let mut db = seeded_db();
    let owner = owner(&mut db);
    let invoice = billed_invoice(&mut db);

    // 2 × 45,000 = 90,000 at 18% → 16,200 tax; 40 × 410 = 16,400 at 28% → 4,592 tax.
    assert_eq!(invoice.subtotal, 106_400.0);
    assert_eq!(invoice.cgst, 10_396.0);
    assert_eq!(invoice.sgst, 10_396.0);
    assert_eq!(invoice.grand_total, 127_192.0);

    let lines = db.creditable_lines(invoice.id).unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].billed_qty, 2.0);
    assert_eq!(lines[0].credited_qty, 0.0);
    assert_eq!(lines[0].creditable_qty, 2.0);

    // One rack comes back.
    let note = db
        .create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            reason: "  Customer returned one rack, damaged in transit  ".into(),
            date: None,
            created_by_user_id: Some(owner.id),
            lines: vec![NewCreditNoteLine { invoice_line_id: lines[0].invoice_line_id, qty: 1.0 }],
        })
        .unwrap();

    assert_eq!(note.credit_note_no, "CN-2026-0001", "its own series, starting at 1");
    assert_eq!(note.reason, "Customer returned one rack, damaged in transit", "trimmed");
    assert_eq!(note.original_invoice_id, invoice.id);
    assert_eq!(note.created_by_user_id, Some(owner.id));

    // Priced as one rack, not as a fraction of the invoice: 45,000 + 9% + 9%.
    assert_eq!(note.subtotal, 45_000.0);
    assert_eq!(note.cgst, 4_050.0);
    assert_eq!(note.sgst, 4_050.0);
    assert_eq!(note.igst, 0.0, "the invoice was intra-state, so the credit is too");
    assert_eq!(note.grand_total, 53_100.0);
    assert_eq!(note.subtotal + note.cgst + note.sgst, note.grand_total);

    // The invoice is untouched. This is the whole point of the design.
    let after = db.get_invoice(invoice.id).unwrap().unwrap();
    assert_eq!(after, invoice, "the original is byte-for-byte what was billed");
    assert_eq!(db.get_invoice_detail(invoice.id).unwrap().unwrap().lines.len(), 2);

    // And the net is the difference.
    let net = db.invoice_net(invoice.id).unwrap();
    assert_eq!(net.note_count, 1);
    assert_eq!(net.credited_total, 53_100.0);
    assert_eq!(net.net_total, 74_092.0, "127,192 billed less 53,100 credited");
}

#[test]
fn a_line_cannot_be_credited_beyond_what_is_left() {
    let mut db = seeded_db();
    let invoice = billed_invoice(&mut db);
    let lines = db.creditable_lines(invoice.id).unwrap();
    let rack_line = lines[0].invoice_line_id;

    // More than was billed, in one go.
    let too_much = db.create_credit_note(&NewCreditNote {
        original_invoice_id: invoice.id,
        reason: "trying it on".into(),
        date: None,
        created_by_user_id: None,
        lines: vec![NewCreditNoteLine { invoice_line_id: rack_line, qty: 3.0 }],
    });
    assert!(matches!(too_much, Err(CoreError::Invalid(_))));

    // Credit one, then try for two more: the ceiling is what *remains*, so two notes
    // cannot add up to more than the invoice however they are ordered.
    db.create_credit_note(&NewCreditNote {
        original_invoice_id: invoice.id,
        reason: "one back".into(),
        date: None,
        created_by_user_id: None,
        lines: vec![NewCreditNoteLine { invoice_line_id: rack_line, qty: 1.0 }],
    })
    .unwrap();

    let after_one = db.creditable_lines(invoice.id).unwrap();
    assert_eq!(after_one[0].credited_qty, 1.0);
    assert_eq!(after_one[0].creditable_qty, 1.0);

    let over = db.create_credit_note(&NewCreditNote {
        original_invoice_id: invoice.id,
        reason: "and another two".into(),
        date: None,
        created_by_user_id: None,
        lines: vec![NewCreditNoteLine { invoice_line_id: rack_line, qty: 2.0 }],
    });
    assert!(matches!(over, Err(CoreError::Invalid(_))), "cannot exceed what is left");

    // The refused note wrote nothing.
    assert_eq!(db.credit_notes_for_invoice(invoice.id).unwrap().len(), 1);
    assert_eq!(db.invoice_net(invoice.id).unwrap().note_count, 1);
}

#[test]
fn a_credit_note_needs_a_reason_and_something_to_credit() {
    let mut db = seeded_db();
    let invoice = billed_invoice(&mut db);
    let lines = db.creditable_lines(invoice.id).unwrap();
    let line = lines[0].invoice_line_id;

    let some = vec![NewCreditNoteLine { invoice_line_id: line, qty: 1.0 }];

    // A correction with no stated reason is the one an auditor asks about and nobody can
    // answer, so there is no path that writes a blank.
    for blank in ["", "   ", "\t\n"] {
        let refused = db.create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            reason: blank.into(),
            date: None,
            created_by_user_id: None,
            lines: some.clone(),
        });
        assert!(matches!(refused, Err(CoreError::Invalid(_))), "blank reason {blank:?}");
    }

    // Nothing selected is not a credit note.
    assert!(matches!(
        db.create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            reason: "fine".into(),
            date: None,
            created_by_user_id: None,
            lines: vec![],
        }),
        Err(CoreError::Invalid(_))
    ));

    // Nor is a set of zero quantities.
    assert!(matches!(
        db.create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            reason: "fine".into(),
            date: None,
            created_by_user_id: None,
            lines: vec![NewCreditNoteLine { invoice_line_id: line, qty: 0.0 }],
        }),
        Err(CoreError::Invalid(_))
    ));

    // A negative quantity would *add* to an invoice through the correction path.
    assert!(matches!(
        db.create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            reason: "fine".into(),
            date: None,
            created_by_user_id: None,
            lines: vec![NewCreditNoteLine { invoice_line_id: line, qty: -1.0 }],
        }),
        Err(CoreError::Invalid(_))
    ));

    // A line from some other invoice is not on this one.
    assert!(matches!(
        db.create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            reason: "fine".into(),
            date: None,
            created_by_user_id: None,
            lines: vec![NewCreditNoteLine { invoice_line_id: 9_999, qty: 1.0 }],
        }),
        Err(CoreError::Invalid(_))
    ));

    assert!(db.credit_notes_for_invoice(invoice.id).unwrap().is_empty(), "nothing was written");
}

#[test]
fn cancelling_an_invoice_is_crediting_every_line_in_full() {
    let mut db = seeded_db();
    let invoice = billed_invoice(&mut db);
    let lines = db.creditable_lines(invoice.id).unwrap();

    // No special path for cancellation: it is the ordinary one, used for everything.
    let note = db
        .create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            reason: "Order cancelled before dispatch".into(),
            date: None,
            created_by_user_id: None,
            lines: lines
                .iter()
                .map(|l| NewCreditNoteLine {
                    invoice_line_id: l.invoice_line_id,
                    qty: l.creditable_qty,
                })
                .collect(),
        })
        .unwrap();

    // A full credit matches the invoice exactly, to the paisa.
    assert_eq!(note.subtotal, invoice.subtotal);
    assert_eq!(note.cgst, invoice.cgst);
    assert_eq!(note.sgst, invoice.sgst);
    assert_eq!(note.igst, invoice.igst);
    assert_eq!(note.grand_total, invoice.grand_total);

    let net = db.invoice_net(invoice.id).unwrap();
    assert_eq!(net.net_total, 0.0, "a cancelled invoice nets to nothing");

    // And there is nothing left to credit twice.
    assert!(db.creditable_lines(invoice.id).unwrap().iter().all(|l| l.creditable_qty == 0.0));
    assert!(db
        .create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            reason: "again".into(),
            date: None,
            created_by_user_id: None,
            lines: vec![NewCreditNoteLine { invoice_line_id: lines[0].invoice_line_id, qty: 1.0 }],
        })
        .is_err());
}

#[test]
fn a_credit_note_reverses_the_tax_that_was_actually_charged() {
    // An inter-state invoice carries IGST, so its credit note must too — even though the
    // home state and the credited items are the same.
    let mut db = seeded_db();
    let buyer = db
        .create_customer(&NewCustomer {
            name: "Deccan Interiors".into(),
            mobile: "9000012345".into(),
            gstin: Some("29AABCS1429B1ZP".into()),
            place_of_supply: "KA".into(),
            price_list_id: None,
            credit_limit: None,
        })
        .unwrap();
    let rack = item(&db, "RACK-42U-PRO");

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "upi".into(),
            created_by_user_id: None,
            lines: vec![NewInvoiceLine {
                item_id: rack.id,
                qty: 1.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
        })
        .unwrap();
    assert!(invoice.igst > 0.0 && invoice.cgst == 0.0, "inter-state");

    let lines = db.creditable_lines(invoice.id).unwrap();
    let note = db
        .create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            reason: "Returned".into(),
            date: None,
            created_by_user_id: None,
            lines: vec![NewCreditNoteLine { invoice_line_id: lines[0].invoice_line_id, qty: 1.0 }],
        })
        .unwrap();

    assert_eq!(note.igst, invoice.igst);
    assert_eq!(note.cgst, 0.0);
    assert_eq!(note.sgst, 0.0);
    assert_eq!(note.grand_total, invoice.grand_total);
}

#[test]
fn credit_notes_run_a_separate_series_and_are_queued_for_sync() {
    let mut db = seeded_db();
    let invoice = billed_invoice(&mut db);
    let second = billed_invoice(&mut db);
    let lines = db.creditable_lines(invoice.id).unwrap();
    let second_lines = db.creditable_lines(second.id).unwrap();

    assert_eq!(invoice.invoice_no, "RI-2026-0001");
    assert_eq!(second.invoice_no, "RI-2026-0002");

    let first_note = db
        .create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            reason: "one".into(),
            date: None,
            created_by_user_id: None,
            lines: vec![NewCreditNoteLine { invoice_line_id: lines[0].invoice_line_id, qty: 1.0 }],
        })
        .unwrap();
    let second_note = db
        .create_credit_note(&NewCreditNote {
            original_invoice_id: second.id,
            reason: "two".into(),
            date: None,
            created_by_user_id: None,
            lines: vec![NewCreditNoteLine {
                invoice_line_id: second_lines[0].invoice_line_id,
                qty: 1.0,
            }],
        })
        .unwrap();

    // Its own counter: two invoices and two notes, numbered independently.
    assert_eq!(first_note.credit_note_no, "CN-2026-0001");
    assert_eq!(second_note.credit_note_no, "CN-2026-0002");

    // Queued like every other mutation, and the note before its lines so the far end
    // never sees a line whose parent has not arrived.
    let queued = db.pending_sync_rows().unwrap();
    let order: Vec<&str> = queued.iter().map(|r| r.table_name.as_str()).collect();
    let note_at = order.iter().position(|t| *t == "credit_notes").unwrap();
    let line_at = order.iter().position(|t| *t == "credit_note_lines").unwrap();
    assert!(note_at < line_at, "a credit note is queued before its lines");

    let payload = &db.pending_sync_rows_for("credit_notes").unwrap()[0].payload_json;
    let value: serde_json::Value = serde_json::from_str(payload).unwrap();
    assert_eq!(value["credit_note_no"], "CN-2026-0001");
    assert_eq!(value["reason"], "one");
    assert_eq!(value["original_invoice_id"], invoice.id);
}

#[test]
fn several_partial_credits_add_up_to_the_whole_and_no_further() {
    let mut db = seeded_db();
    let invoice = billed_invoice(&mut db);
    let cement_line = db.creditable_lines(invoice.id).unwrap()[1].invoice_line_id;

    // 40 bags back in four lots of ten.
    for lot in 1..=4 {
        db.create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            reason: format!("return lot {lot}"),
            date: None,
            created_by_user_id: None,
            lines: vec![NewCreditNoteLine { invoice_line_id: cement_line, qty: 10.0 }],
        })
        .unwrap();
    }

    let lines = db.creditable_lines(invoice.id).unwrap();
    assert_eq!(lines[1].credited_qty, 40.0);
    assert_eq!(lines[1].creditable_qty, 0.0);

    // The four notes together equal that line's share of the invoice: 40 × 410 at 28%,
    // which is 16,400 + 4,592. Crediting the cement must not pick up the racks' 18%.
    let net = db.invoice_net(invoice.id).unwrap();
    assert_eq!(net.note_count, 4);
    assert_eq!(net.credited_subtotal, 16_400.0);
    assert_eq!(net.credited_total, 20_992.0);
    // The racks were never credited, so the net is exactly their share.
    assert_eq!(net.net_total, 106_200.0, "127,192 billed less 20,992 credited");

    assert!(db
        .create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            reason: "one bag too many".into(),
            date: None,
            created_by_user_id: None,
            lines: vec![NewCreditNoteLine { invoice_line_id: cement_line, qty: 1.0 }],
        })
        .is_err());
}

#[test]
fn reporting_nets_credit_notes_against_revenue() {
    let mut db = seeded_db();
    let invoice = billed_invoice(&mut db);
    let lines = db.creditable_lines(invoice.id).unwrap();

    let gross = db.sales_summary(&DateRange::default()).unwrap();
    assert_eq!(gross.grand_total, 127_192.0);
    assert_eq!(gross.credit_note_count, 0);
    assert_eq!(gross.net_total, gross.grand_total, "nothing credited yet");

    // One rack back: 45,000 + 18%.
    db.create_credit_note(&NewCreditNote {
        original_invoice_id: invoice.id,
        reason: "returned".into(),
        date: None,
        created_by_user_id: None,
        lines: vec![NewCreditNoteLine { invoice_line_id: lines[0].invoice_line_id, qty: 1.0 }],
    })
    .unwrap();

    let netted = db.sales_summary(&DateRange::default()).unwrap();

    // What was billed is unchanged — both figures are real, and a report that quietly
    // rewrote the gross would be answering a different question from the one asked.
    assert_eq!(netted.invoice_count, 1);
    assert_eq!(netted.grand_total, 127_192.0);
    assert_eq!(netted.subtotal, 106_400.0);

    // And what was earned is what is left.
    assert_eq!(netted.credit_note_count, 1);
    assert_eq!(netted.credited_subtotal, 45_000.0);
    assert_eq!(netted.credited_tax, 8_100.0);
    assert_eq!(netted.credited_total, 53_100.0);
    assert_eq!(netted.net_subtotal, 61_400.0);
    assert_eq!(netted.net_tax, 12_692.0);
    assert_eq!(netted.net_total, 74_092.0);
    assert_eq!(netted.net_subtotal + netted.net_tax, netted.net_total, "the net reconciles");

    // The day's row nets too.
    let days = db.daily_totals(&DateRange::default()).unwrap();
    assert_eq!(days.len(), 1);
    assert_eq!(days[0].grand_total, 127_192.0);
    assert_eq!(days[0].credited_total, 53_100.0);
    assert_eq!(days[0].net_total, 74_092.0);

    // The payment mix nets against the invoice's own payment type.
    let mix = db.payment_mix(&DateRange::default()).unwrap();
    assert_eq!(mix.len(), 1);
    assert_eq!(mix[0].payment_type, "cash");
    assert_eq!(mix[0].grand_total, 74_092.0);

    // And "what sold" counts one rack, not two: an item sold and returned is not a sale.
    let top = db.top_items(&DateRange::default(), 8).unwrap();
    let rack = top.iter().find(|t| t.item_code == "RACK-42U-PRO").unwrap();
    assert_eq!(rack.qty, 1.0, "2 billed less 1 credited");
    assert_eq!(rack.revenue, 45_000.0);
    assert_eq!(rack.credited_qty, 1.0);
    assert_eq!(rack.credited_revenue, 45_000.0);

    // The cement was untouched, so it is unchanged.
    let cement = top.iter().find(|t| t.item_code == "CEM-OPC-53").unwrap();
    assert_eq!(cement.qty, 40.0);
    assert_eq!(cement.credited_qty, 0.0);
}

#[test]
fn a_fully_cancelled_invoice_nets_to_nothing_in_reporting() {
    let mut db = seeded_db();
    let invoice = billed_invoice(&mut db);
    let lines = db.creditable_lines(invoice.id).unwrap();

    db.create_credit_note(&NewCreditNote {
        original_invoice_id: invoice.id,
        reason: "cancelled".into(),
        date: None,
        created_by_user_id: None,
        lines: lines
            .iter()
            .map(|l| NewCreditNoteLine { invoice_line_id: l.invoice_line_id, qty: l.billed_qty })
            .collect(),
    })
    .unwrap();

    let summary = db.sales_summary(&DateRange::default()).unwrap();
    assert_eq!(summary.net_total, 0.0);
    assert_eq!(summary.net_subtotal, 0.0);
    assert_eq!(summary.net_tax, 0.0, "no tax is owed on a sale that was undone");
    assert_eq!(summary.grand_total, 127_192.0, "but the invoice still happened");

    // Nothing sold on net, so nothing ranks above zero.
    let top = db.top_items(&DateRange::default(), 8).unwrap();
    assert!(top.iter().all(|t| t.revenue == 0.0 && t.qty == 0.0));

    assert_eq!(db.payment_mix(&DateRange::default()).unwrap()[0].grand_total, 0.0);
    assert_eq!(db.daily_totals(&DateRange::default()).unwrap()[0].net_total, 0.0);
}

// -------------------------------------------------------------- one-off items

#[test]
fn a_one_off_item_bills_without_joining_the_catalogue() {
    let mut db = seeded_db();
    let before = db.count_items().unwrap();

    let delivery = db.create_custom_item("Delivery to site", 750.0, 18.0, "").unwrap();
    assert!(delivery.custom);
    assert_eq!(delivery.uom, "NOS", "a unit is never blank on a bill");
    assert!(delivery.item_code.starts_with("ONE-OFF-"), "{}", delivery.item_code);

    // Invisible to the price list a shop maintains, and to the billing picker, so it
    // cannot be picked again by accident on the next sale.
    assert_eq!(db.count_items().unwrap(), before, "the catalogue count is unchanged");
    assert!(db.list_items(&ItemFilter::default()).unwrap().iter().all(|i| !i.custom));
    assert!(db
        .list_items(&ItemFilter { text: Some("Delivery".into()), ..Default::default() })
        .unwrap()
        .is_empty());
    assert!(db.search_item("Delivery").unwrap().is_empty());
    assert!(db.search_item("").unwrap().iter().all(|i| !i.custom));

    // But it bills like anything else, and prices through the same GST code.
    let buyer = customer(&db, "9840012345");
    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            lines: vec![NewInvoiceLine {
                item_id: delivery.id,
                qty: 2.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
        })
        .unwrap();

    assert_eq!(invoice.subtotal, 1_500.0);
    assert_eq!(invoice.cgst, 135.0);
    assert_eq!(invoice.sgst, 135.0);
    assert_eq!(invoice.grand_total, 1_770.0);

    // And it reads back on the invoice with its description, tagged as custom so the
    // screen can say which lines came from the catalogue.
    let detail = db.get_invoice_detail(invoice.id).unwrap().unwrap();
    assert_eq!(detail.lines.len(), 1);
    assert_eq!(detail.lines[0].description, "Delivery to site");
    assert!(db.get_item(delivery.id).unwrap().unwrap().custom);
}

#[test]
fn a_one_off_item_refuses_nonsense() {
    let mut db = seeded_db();

    assert!(db.create_custom_item("   ", 100.0, 18.0, "NOS").is_err(), "no description");
    assert!(db.create_custom_item("Fitting", -1.0, 18.0, "NOS").is_err(), "negative rate");
    assert!(db.create_custom_item("Fitting", f64::NAN, 18.0, "NOS").is_err());
    assert!(db.create_custom_item("Fitting", 100.0, 900.0, "NOS").is_err(), "tax over 100");
    assert!(db.create_custom_item("Fitting", 100.0, -5.0, "NOS").is_err());

    // Both boundaries are legitimate: zero-rated goods, and a free delivery.
    assert!(db.create_custom_item("Free delivery", 0.0, 0.0, "NOS").is_ok());
    assert!(db.create_custom_item("Fully taxed", 100.0, 100.0, "NOS").is_ok());

    // Two one-offs with the same description are two different items, not a collision.
    let first = db.create_custom_item("Site visit", 500.0, 18.0, "NOS").unwrap();
    let second = db.create_custom_item("Site visit", 500.0, 18.0, "NOS").unwrap();
    assert_ne!(first.id, second.id);
    assert_ne!(first.item_code, second.item_code);
}

// ------------------------------------------------------------ clearing demo data

#[test]
fn clearing_demo_data_removes_untouched_samples_only() {
    let mut db = seeded_db();
    assert_eq!(db.count_items().unwrap(), 7);
    assert!(db.search_customer("9791045678").unwrap().is_some(), "a sample customer exists");

    // Bill one demo item to one demo customer, so both are no longer untouched.
    let buyer = customer(&db, "9840012345");
    let rack = item(&db, "RACK-42U-PRO");
    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            lines: vec![NewInvoiceLine {
                item_id: rack.id,
                qty: 1.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
        })
        .unwrap();

    let cleared = db.clear_demo_data().unwrap();

    // The six unbilled items and four unbilled customers go.
    assert_eq!(cleared.items_removed, 6);
    assert_eq!(cleared.items_kept, 1, "the billed rack stays");
    assert_eq!(cleared.customers_removed, 4);
    assert_eq!(cleared.customers_kept, 1, "the billed customer stays");

    // What is left is exactly what the invoice still needs.
    assert_eq!(db.count_items().unwrap(), 1);
    assert!(db.search_item("ABCOS").unwrap().is_empty(), "the sample licence is gone");
    assert!(db.search_customer("9791045678").unwrap().is_none(), "an unbilled sample is gone");

    // And the invoice is completely intact and still readable, which is the whole
    // constraint: clearing sample data must never cost a real record.
    let after = db.get_invoice(invoice.id).unwrap().unwrap();
    assert_eq!(after, invoice);
    let detail = db.get_invoice_detail(invoice.id).unwrap().unwrap();
    assert_eq!(detail.lines.len(), 1);
    assert_eq!(detail.lines[0].item_code, "RACK-42U-PRO");
    assert_eq!(detail.customer.mobile, "9840012345");
    assert_eq!(db.list_invoices(&InvoiceFilter::default()).unwrap().len(), 1);
}

#[test]
fn clearing_demo_data_twice_is_harmless_and_spares_real_rows() {
    let mut db = seeded_db();

    // A real item a shop added themselves, and a real customer.
    let real = db
        .upsert_item(&NewItem {
            item_code: "PATCH-CAT6".into(),
            description: "Cat6 patch cable".into(),
            rate: 180.0,
            tax_rate: 18.0,
            uom: "NOS".into(),
            custom: false,
        })
        .unwrap();
    let real_customer = db
        .create_customer(&NewCustomer {
            name: "Deccan Interiors".into(),
            mobile: "9000012345".into(),
            gstin: None,
            place_of_supply: "TN".into(),
            price_list_id: None,
            credit_limit: None,
        })
        .unwrap();

    let first = db.clear_demo_data().unwrap();
    assert_eq!(first.items_removed, 7);
    assert_eq!(first.customers_removed, 5);

    // Run again on an already-clean install: nothing to do, and no error.
    let second = db.clear_demo_data().unwrap();
    assert_eq!(second.items_removed, 0);
    assert_eq!(second.customers_removed, 0);
    assert_eq!(second.items_kept, 0);

    // Nothing the shop made themselves was touched.
    assert!(db.get_item(real.id).unwrap().is_some());
    assert!(db.search_customer(&real_customer.mobile).unwrap().is_some());
    assert_eq!(db.count_items().unwrap(), 1);
}

// ============================================================== price lists

/// The migration leaves a database that prices exactly as it did before it ran.
#[test]
fn an_existing_database_lands_on_a_default_list_at_its_existing_prices() {
    let db = seeded_db();

    let lists = db.price_lists().unwrap();
    assert_eq!(lists.len(), 1, "one list to start with");
    assert_eq!(lists[0].name, "Retail");
    assert!(lists[0].is_default);
    assert_eq!(db.default_price_list().unwrap().id, lists[0].id);

    // Every seeded item now has that price stated rather than assumed, and it is the
    // same number `items.rate` always held.
    for item in db.search_item("").unwrap() {
        let resolved = db.resolve_rate(item.id, lists[0].id).unwrap();
        assert_eq!(resolved.rate, item.rate, "{} priced differently", item.item_code);
        assert!(resolved.from_price_list, "{} should have a Retail entry", item.item_code);
    }

    // And a customer nobody has assigned is billed from it.
    let buyer = customer(&db, "9840012345");
    assert_eq!(buyer.price_list_id, None);
    assert_eq!(db.price_list_for_customer(buyer.id).unwrap().id, lists[0].id);
}

/// A one-off is not a price list entry. It is one line on one bill.
#[test]
fn a_one_off_item_gets_no_price_list_entry_and_keeps_its_typed_rate() {
    let mut db = seeded_db();
    let retail = db.default_price_list().unwrap();
    let wholesale = db.create_price_list("Wholesale").unwrap();

    let one_off =
        db.create_custom_item("Shutter spring replacement", 1_450.0, 18.0, "NOS").unwrap();

    for list in [&retail, &wholesale] {
        let resolved = db.resolve_rate(one_off.id, list.id).unwrap();
        assert_eq!(resolved.rate, 1_450.0);
        assert!(!resolved.from_price_list, "a one-off falls back on every list");
    }
    assert!(db.item_prices(one_off.id).unwrap().iter().all(|row| row.rate.is_none()));
}

/// The headline case: same item, two customers, two rates.
#[test]
fn two_customers_on_different_lists_are_billed_different_rates_for_the_same_item() {
    let mut db = seeded_db();
    let retail = db.default_price_list().unwrap();
    let wholesale = db.create_price_list("Wholesale").unwrap();

    let cement = item(&db, "CEM-OPC-53"); // 410.00 at 28%
    db.set_item_prices(
        cement.id,
        &[
            ItemPrice { item_id: cement.id, price_list_id: retail.id, rate: 410.0 },
            ItemPrice { item_id: cement.id, price_list_id: wholesale.id, rate: 365.0 },
        ],
    )
    .unwrap();

    let builder = db
        .create_customer(&NewCustomer {
            name: "Kavi Constructions".into(),
            mobile: "9111122233".into(),
            gstin: None,
            place_of_supply: "TN".into(),
            price_list_id: Some(wholesale.id),
            credit_limit: None,
        })
        .unwrap();
    let walk_in = customer(&db, "9840012345"); // left on the default

    // Neither invoice states a rate. That is the point: the rate is the database's to
    // decide from who is being billed, not the caller's to assert.
    let line = |item_id| NewInvoiceLine {
        item_id,
        qty: 100.0,
        rate: None,
        tax_rate: None,
        discount_type: DiscountType::None,
        discount_value: 0.0,
    };

    let trade = db
        .create_invoice(&NewInvoice {
            customer_id: builder.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            lines: vec![line(cement.id)],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
        })
        .unwrap();
    let counter = db
        .create_invoice(&NewInvoice {
            customer_id: walk_in.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            lines: vec![line(cement.id)],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
        })
        .unwrap();

    assert_eq!(trade.subtotal, 36_500.00, "100 bags at the wholesale 365.00");
    assert_eq!(counter.subtotal, 41_000.00, "100 bags at the retail 410.00");

    // Taxed on what was actually charged, not on a single sticker price.
    assert_eq!(trade.grand_total, 46_720.00); // 36,500 + 28%
    assert_eq!(counter.grand_total, 52_480.00); // 41,000 + 28%

    // The rate is on the line, so each invoice stays readable on its own terms.
    assert_eq!(db.invoice_lines(trade.id).unwrap()[0].rate, 365.0);
    assert_eq!(db.invoice_lines(counter.id).unwrap()[0].rate, 410.0);
}

/// A list only has to price what the shop actually discounts.
#[test]
fn an_item_with_no_entry_on_the_list_falls_back_to_its_base_rate() {
    let mut db = seeded_db();
    let wholesale = db.create_price_list("Wholesale").unwrap();

    let cement = item(&db, "CEM-OPC-53");
    let steel = item(&db, "TMT-12MM"); // deliberately left unpriced on Wholesale
    db.set_item_prices(
        cement.id,
        &[ItemPrice { item_id: cement.id, price_list_id: wholesale.id, rate: 365.0 }],
    )
    .unwrap();

    let priced = db.resolve_rate(cement.id, wholesale.id).unwrap();
    assert_eq!(priced.rate, 365.0);
    assert!(priced.from_price_list);

    let fell_back = db.resolve_rate(steel.id, wholesale.id).unwrap();
    assert_eq!(fell_back.rate, steel.rate, "620.00, the base rate");
    assert!(!fell_back.from_price_list, "and the screen can say so");

    let builder = db
        .create_customer(&NewCustomer {
            name: "Kavi Constructions".into(),
            mobile: "9111122233".into(),
            gstin: None,
            place_of_supply: "TN".into(),
            price_list_id: Some(wholesale.id),
            credit_limit: None,
        })
        .unwrap();

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: builder.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            lines: vec![
                NewInvoiceLine {
                    item_id: cement.id,
                    qty: 10.0,
                    rate: None,
                    tax_rate: None,
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                },
                NewInvoiceLine {
                    item_id: steel.id,
                    qty: 10.0,
                    rate: None,
                    tax_rate: None,
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                },
            ],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
        })
        .unwrap();

    let lines = db.invoice_lines(invoice.id).unwrap();
    assert_eq!(lines[0].rate, 365.0, "the wholesale rate");
    assert_eq!(lines[1].rate, 620.0, "and the base rate beside it on the same bill");
}

/// "Exactly one default" is the database's rule, not a convention.
#[test]
fn the_default_flag_moves_and_never_splits_or_disappears() {
    let mut db = seeded_db();
    let retail = db.default_price_list().unwrap();
    let wholesale = db.create_price_list("Wholesale").unwrap();
    assert!(!wholesale.is_default, "a new list does not seize the default");

    let now_default = db.set_default_price_list(wholesale.id).unwrap();
    assert!(now_default.is_default);

    let flagged: Vec<_> = db.price_lists().unwrap().into_iter().filter(|l| l.is_default).collect();
    assert_eq!(flagged.len(), 1, "one default, always");
    assert_eq!(flagged[0].id, wholesale.id);
    assert!(!db.get_price_list(retail.id).unwrap().unwrap().is_default, "the old one stood down");

    // And an unassigned customer follows the flag rather than being pinned to the list
    // that happened to be default when they were registered.
    let walk_in = customer(&db, "9840012345");
    assert_eq!(walk_in.price_list_id, None);
    assert_eq!(db.price_list_for_customer(walk_in.id).unwrap().id, wholesale.id);
}

#[test]
fn price_lists_are_named_once_and_renamed_without_touching_their_rates() {
    let mut db = seeded_db();
    let wholesale = db.create_price_list("Wholesale").unwrap();
    let cement = item(&db, "CEM-OPC-53");
    db.set_item_prices(
        cement.id,
        &[ItemPrice { item_id: cement.id, price_list_id: wholesale.id, rate: 365.0 }],
    )
    .unwrap();

    assert!(db.create_price_list("  ").is_err(), "a list needs a name");
    assert!(db.create_price_list("wholesale").is_err(), "and names are unique, case aside");

    let renamed = db.rename_price_list(wholesale.id, "Trade").unwrap();
    assert_eq!(renamed.name, "Trade");
    assert_eq!(renamed.id, wholesale.id);
    assert_eq!(
        db.resolve_rate(cement.id, wholesale.id).unwrap().rate,
        365.0,
        "renaming a list is not repricing it"
    );
    assert!(db.rename_price_list(wholesale.id, "Retail").is_err(), "onto another list's name");
    assert!(db.rename_price_list(9_999, "Anything").is_err(), "or a list that does not exist");
}

/// Clearing a box on the form means "fall back", not "leave it alone".
#[test]
fn setting_prices_replaces_the_whole_table_for_that_item() {
    let mut db = seeded_db();
    let retail = db.default_price_list().unwrap();
    let wholesale = db.create_price_list("Wholesale").unwrap();
    let cement = item(&db, "CEM-OPC-53");

    db.set_item_prices(
        cement.id,
        &[
            ItemPrice { item_id: cement.id, price_list_id: retail.id, rate: 410.0 },
            ItemPrice { item_id: cement.id, price_list_id: wholesale.id, rate: 365.0 },
        ],
    )
    .unwrap();

    // Submitting the form with Wholesale blanked removes that entry.
    db.set_item_prices(
        cement.id,
        &[ItemPrice { item_id: cement.id, price_list_id: retail.id, rate: 415.0 }],
    )
    .unwrap();

    let rows = db.item_prices(cement.id).unwrap();
    let retail_row = rows.iter().find(|r| r.price_list.id == retail.id).unwrap();
    let wholesale_row = rows.iter().find(|r| r.price_list.id == wholesale.id).unwrap();
    assert_eq!(retail_row.rate, Some(415.0));
    assert_eq!(wholesale_row.rate, None, "blanked, so it falls back");
    assert_eq!(db.resolve_rate(cement.id, wholesale.id).unwrap().rate, cement.rate);

    assert!(
        db.set_item_prices(
            cement.id,
            &[ItemPrice { item_id: cement.id, price_list_id: retail.id, rate: -1.0 }]
        )
        .is_err(),
        "a negative price is a typo"
    );
    assert!(
        db.set_item_prices(
            cement.id,
            &[ItemPrice { item_id: cement.id, price_list_id: 9_999, rate: 1.0 }]
        )
        .is_err(),
        "and so is a list that does not exist"
    );
}

/// Moving a customer between lists changes what they pay next, never what they paid.
#[test]
fn reassigning_a_customer_leaves_their_old_invoices_exactly_as_billed() {
    let mut db = seeded_db();
    let wholesale = db.create_price_list("Wholesale").unwrap();
    let cement = item(&db, "CEM-OPC-53");
    db.set_item_prices(
        cement.id,
        &[ItemPrice { item_id: cement.id, price_list_id: wholesale.id, rate: 365.0 }],
    )
    .unwrap();

    let buyer = customer(&db, "9840012345");
    let before = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            lines: vec![NewInvoiceLine {
                item_id: cement.id,
                qty: 10.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
        })
        .unwrap();
    assert_eq!(before.subtotal, 4_100.00);

    let moved = db.set_customer_price_list(buyer.id, Some(wholesale.id)).unwrap();
    assert_eq!(moved.price_list_id, Some(wholesale.id));
    assert_eq!(db.price_list_for_customer(buyer.id).unwrap().id, wholesale.id);

    // The invoice already raised is untouched, in the row and in its lines.
    assert_eq!(db.get_invoice(before.id).unwrap().unwrap(), before);
    assert_eq!(db.invoice_lines(before.id).unwrap()[0].rate, 410.0);

    // The next one is billed at the new list.
    let after = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            lines: vec![NewInvoiceLine {
                item_id: cement.id,
                qty: 10.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
        })
        .unwrap();
    assert_eq!(after.subtotal, 3_650.00);

    // And back to the default with None.
    let reset = db.set_customer_price_list(buyer.id, None).unwrap();
    assert_eq!(reset.price_list_id, None);
    assert!(db.set_customer_price_list(buyer.id, Some(9_999)).is_err());
}

/// An explicit rate still wins, which is the hook a per-line discount will use.
#[test]
fn an_explicit_rate_overrides_the_price_list() {
    let mut db = seeded_db();
    let wholesale = db.create_price_list("Wholesale").unwrap();
    let cement = item(&db, "CEM-OPC-53");
    db.set_item_prices(
        cement.id,
        &[ItemPrice { item_id: cement.id, price_list_id: wholesale.id, rate: 365.0 }],
    )
    .unwrap();
    let buyer =
        db.set_customer_price_list(customer(&db, "9840012345").id, Some(wholesale.id)).unwrap();

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            lines: vec![NewInvoiceLine {
                item_id: cement.id,
                qty: 10.0,
                rate: Some(350.0),
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
        })
        .unwrap();

    assert_eq!(invoice.subtotal, 3_500.00);
    assert_eq!(db.invoice_lines(invoice.id).unwrap()[0].rate, 350.0);
}

/// The picker shows the rate that will be billed, not the one on the sticker.
#[test]
fn the_priced_search_resolves_every_row_for_the_list_it_is_given() {
    let mut db = seeded_db();
    let retail = db.default_price_list().unwrap();
    let wholesale = db.create_price_list("Wholesale").unwrap();
    let cement = item(&db, "CEM-OPC-53");
    db.set_item_prices(
        cement.id,
        &[ItemPrice { item_id: cement.id, price_list_id: wholesale.id, rate: 365.0 }],
    )
    .unwrap();

    let on_retail = db.search_item_priced("CEM-OPC-53", retail.id).unwrap();
    assert_eq!(on_retail.len(), 1);
    assert_eq!(on_retail[0].rate, 410.0);
    assert!(!on_retail[0].from_price_list, "the Retail entry went when the table was replaced");

    let on_wholesale = db.search_item_priced("CEM-OPC-53", wholesale.id).unwrap();
    assert_eq!(on_wholesale[0].rate, 365.0);
    assert!(on_wholesale[0].from_price_list);
    assert_eq!(on_wholesale[0].item.rate, 410.0, "the base rate is still reported as itself");

    // One-offs stay out of the picker whichever list is being priced.
    let one_off = db.create_custom_item("Crane hire", 9_000.0, 18.0, "DAY").unwrap();
    assert!(db
        .search_item_priced("", wholesale.id)
        .unwrap()
        .iter()
        .all(|p| p.item.id != one_off.id));
}

// ================================================================ discounts

/// The stage's worked example, billed end to end, against the same hand calculation the
/// GST unit test asserts — but through `create_invoice`, so what is *stored* is checked
/// as well as what is computed.
///
/// 1 x 45,000 @18% and 5 x 12,000 @18% with 10% off the second line, then a flat 1,000
/// off the invoice, comes to 98,000.00 taxable and 115,640.00 all in.
#[test]
fn an_invoice_with_line_and_invoice_discounts_stores_the_hand_calculated_figures() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let rack = item(&db, "RACK-42U-PRO");
    let license = item(&db, "ABCOS-ENT-LIC");

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            invoice_discount_type: DiscountType::Flat,
            invoice_discount_value: 1_000.0,
            lines: vec![
                NewInvoiceLine {
                    item_id: rack.id,
                    qty: 1.0,
                    rate: Some(45_000.0),
                    tax_rate: Some(18.0),
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                },
                NewInvoiceLine {
                    item_id: license.id,
                    qty: 5.0,
                    rate: Some(12_000.0),
                    tax_rate: Some(18.0),
                    discount_type: DiscountType::Percentage,
                    discount_value: 10.0,
                },
            ],
        })
        .unwrap();

    assert_eq!(invoice.pre_discount_subtotal, 105_000.00, "what it would have cost");
    assert_eq!(invoice.discount_amount, 7_000.00, "6,000 line + 1,000 invoice");
    assert_eq!(invoice.invoice_discount_amount, 1_000.00);
    assert_eq!(invoice.invoice_discount_type, DiscountType::Flat);
    assert_eq!(invoice.invoice_discount_value, 1_000.0);
    assert_eq!(invoice.subtotal, 98_000.00, "the taxable value");
    assert_eq!(invoice.cgst, 8_820.00);
    assert_eq!(invoice.sgst, 8_820.00);
    assert_eq!(invoice.igst, 0.0);
    assert_eq!(invoice.grand_total, 115_640.00);

    // Every figure the tax was worked out from is on the line, not re-derivable-only.
    let lines = db.invoice_lines(invoice.id).unwrap();
    assert_eq!(lines[0].line_total, 45_000.00);
    assert_eq!(lines[0].discount_type, DiscountType::None);
    assert_eq!(lines[0].discount_amount, 0.0);
    assert_eq!(lines[0].invoice_discount_share, 454.55);
    assert_eq!(lines[0].taxable_value, 44_545.45);

    assert_eq!(lines[1].line_total, 60_000.00);
    assert_eq!(lines[1].discount_type, DiscountType::Percentage);
    assert_eq!(lines[1].discount_value, 10.0);
    assert_eq!(lines[1].discount_amount, 6_000.00);
    assert_eq!(lines[1].invoice_discount_share, 545.45);
    assert_eq!(lines[1].taxable_value, 53_454.55);

    // The parts add up to the whole, which is the property an auditor checks first.
    let taxable: f64 = lines.iter().map(|l| l.taxable_value).sum();
    assert_eq!(taxable, invoice.subtotal);
    let gross: f64 = lines.iter().map(|l| l.line_total).sum();
    assert_eq!(gross, invoice.pre_discount_subtotal);
    let discounts: f64 = lines.iter().map(|l| l.discount_amount + l.invoice_discount_share).sum();
    assert_eq!(discounts, invoice.discount_amount);
}

/// The whole point of freezing the figures: a saved invoice must not move when the
/// catalogue does.
#[test]
fn a_discounted_invoice_is_unchanged_by_a_later_price_change() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let cement = item(&db, "CEM-OPC-53"); // 410.00 at 28%

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            invoice_discount_type: DiscountType::Percentage,
            invoice_discount_value: 5.0,
            lines: vec![NewInvoiceLine {
                item_id: cement.id,
                qty: 100.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::Percentage,
                discount_value: 10.0,
            }],
        })
        .unwrap();

    // 41,000 less 10% = 36,900; less 5% = 35,055; 28% of that is 9,815.40.
    assert_eq!(invoice.pre_discount_subtotal, 41_000.00);
    assert_eq!(invoice.discount_amount, 5_945.00);
    assert_eq!(invoice.subtotal, 35_055.00);
    assert_eq!(invoice.grand_total, 44_870.40);

    let before = db.get_invoice_detail(invoice.id).unwrap().unwrap();

    // Now move the catalogue underneath it — a new base rate and a new price list entry.
    let wholesale = db.create_price_list("Wholesale").unwrap();
    db.set_item_prices(
        cement.id,
        &[ItemPrice { item_id: cement.id, price_list_id: wholesale.id, rate: 200.0 }],
    )
    .unwrap();
    db.upsert_item(&NewItem {
        item_code: "CEM-OPC-53".into(),
        description: "OPC 53 Grade Cement".into(),
        rate: 999.0,
        tax_rate: 28.0,
        uom: "BAG".into(),
        custom: false,
    })
    .unwrap();
    db.set_customer_price_list(buyer.id, Some(wholesale.id)).unwrap();

    let after = db.get_invoice_detail(invoice.id).unwrap().unwrap();
    assert_eq!(after.invoice, before.invoice, "not one figure moved");
    assert_eq!(after.lines, before.lines);
    assert_eq!(after.invoice.grand_total, 44_870.40);
    assert_eq!(after.lines[0].line.rate, 410.0, "still the rate it was billed at");
    assert_eq!(after.lines[0].line.taxable_value, 35_055.00);
}

/// A return on a discounted line refunds what was paid, not the sticker price.
#[test]
fn a_credit_note_on_a_discounted_line_credits_the_discounted_price() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let cement = item(&db, "CEM-OPC-53"); // 410.00 at 28%

    // 100 bags at 410 = 41,000, less 20% = 32,800. So each bag really cost 328.00.
    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
            lines: vec![NewInvoiceLine {
                item_id: cement.id,
                qty: 100.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::Percentage,
                discount_value: 20.0,
            }],
        })
        .unwrap();
    assert_eq!(invoice.subtotal, 32_800.00);

    let creditable = db.creditable_lines(invoice.id).unwrap();
    assert_eq!(creditable[0].rate, 410.0, "the billed rate, for the customer's copy");
    assert_eq!(creditable[0].effective_rate, 328.0, "what a bag actually cost");

    // Ten bags come back.
    let note = db
        .create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            date: None,
            reason: "Ten bags returned, damaged".into(),
            created_by_user_id: None,
            lines: vec![NewCreditNoteLine {
                invoice_line_id: creditable[0].invoice_line_id,
                qty: 10.0,
            }],
        })
        .unwrap();

    // 10 x 328.00 = 3,280.00, and 28% of that is 918.40 — not 10 x 410.
    assert_eq!(note.subtotal, 3_280.00);
    assert_eq!(note.cgst, 459.20);
    assert_eq!(note.sgst, 459.20);
    assert_eq!(note.grand_total, 4_198.40);

    // The whole line credited back comes to exactly what the line was billed.
    let rest = db
        .create_credit_note(&NewCreditNote {
            original_invoice_id: invoice.id,
            date: None,
            reason: "The other ninety too".into(),
            created_by_user_id: None,
            lines: vec![NewCreditNoteLine {
                invoice_line_id: creditable[0].invoice_line_id,
                qty: 90.0,
            }],
        })
        .unwrap();
    assert_eq!(
        gst_round(note.grand_total + rest.grand_total),
        invoice.grand_total,
        "crediting everything nets the invoice to zero"
    );
    assert_eq!(db.invoice_net(invoice.id).unwrap().net_total, 0.0);
}

fn gst_round(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Core refuses what the screen should never have sent, so a bad discount cannot be
/// talked past the UI by a hand-made payload.
#[test]
fn a_discount_bigger_than_the_bill_is_refused_and_writes_nothing() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let cement = item(&db, "CEM-OPC-53");

    let bad = db.create_invoice(&NewInvoice {
        customer_id: buyer.id,
        date: None,
        payment_type: "cash".into(),
        created_by_user_id: None,
        invoice_discount_type: DiscountType::Flat,
        invoice_discount_value: 999_999.0,
        lines: vec![NewInvoiceLine {
            item_id: cement.id,
            qty: 1.0,
            rate: None,
            tax_rate: None,
            discount_type: DiscountType::None,
            discount_value: 0.0,
        }],
    });
    assert!(matches!(bad, Err(CoreError::Invalid(_))), "{bad:?}");

    // Nothing was written, and the invoice number was not burnt.
    assert!(db.list_todays_invoices().unwrap().is_empty());
    assert!(db.pending_sync_rows_for("invoices").unwrap().is_empty());
    assert!(db.pending_sync_rows_for("invoice_lines").unwrap().is_empty());
}

/// Appending to an invoice that carries a discount re-apportions it across every line,
/// rather than leaving the earlier lines holding shares of a subtotal that has changed.
#[test]
fn adding_a_line_reapportions_the_invoice_discount_over_all_of_them() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let rack = item(&db, "RACK-42U-PRO"); // 45,000 @ 18%
    let steel = item(&db, "TMT-12MM"); // 620 @ 18%

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            invoice_discount_type: DiscountType::Flat,
            invoice_discount_value: 1_000.0,
            lines: vec![NewInvoiceLine {
                item_id: rack.id,
                qty: 1.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
        })
        .unwrap();

    // One line, so it carries the whole discount.
    assert_eq!(db.invoice_lines(invoice.id).unwrap()[0].invoice_discount_share, 1_000.00);
    assert_eq!(invoice.subtotal, 44_000.00);

    db.add_line_item(
        invoice.id,
        &NewInvoiceLine {
            item_id: steel.id,
            qty: 10.0,
            rate: None,
            tax_rate: None,
            discount_type: DiscountType::None,
            discount_value: 0.0,
        },
    )
    .unwrap();

    // 45,000 + 6,200 = 51,200 to share 1,000 across.
    let lines = db.invoice_lines(invoice.id).unwrap();
    let shares: Vec<f64> = lines.iter().map(|l| l.invoice_discount_share).collect();
    assert_eq!(shares, vec![878.91, 121.09], "the first line's share came down");
    assert_eq!(gst_round(shares.iter().sum::<f64>()), 1_000.00);

    let updated = db.get_invoice(invoice.id).unwrap().unwrap();
    assert_eq!(updated.pre_discount_subtotal, 51_200.00);
    assert_eq!(updated.discount_amount, 1_000.00);
    assert_eq!(updated.subtotal, 50_200.00);
    assert_eq!(gst_round(lines.iter().map(|l| l.taxable_value).sum::<f64>()), updated.subtotal);
}

/// An undiscounted invoice states its pre-discount subtotal as its subtotal, so the two
/// columns the reports will read are always both populated.
#[test]
fn an_undiscounted_invoice_reports_no_discount_rather_than_a_blank() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let cement = item(&db, "CEM-OPC-53");

    let invoice = db
        .create_invoice(&NewInvoice {
            customer_id: buyer.id,
            date: None,
            payment_type: "cash".into(),
            created_by_user_id: None,
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
            lines: vec![NewInvoiceLine {
                item_id: cement.id,
                qty: 10.0,
                rate: None,
                tax_rate: None,
                discount_type: DiscountType::None,
                discount_value: 0.0,
            }],
        })
        .unwrap();

    assert_eq!(invoice.pre_discount_subtotal, 4_100.00);
    assert_eq!(invoice.subtotal, 4_100.00);
    assert_eq!(invoice.discount_amount, 0.0);
    assert_eq!(invoice.invoice_discount_amount, 0.0);
    assert_eq!(invoice.invoice_discount_type, DiscountType::None);

    let line = &db.invoice_lines(invoice.id).unwrap()[0];
    assert_eq!(line.taxable_value, line.line_total);
    assert_eq!(line.discount_amount, 0.0);
    assert_eq!(line.invoice_discount_share, 0.0);
}

// ================================================================== ledger

/// A helper that bills `total` worth to a customer on the given terms.
fn bill(
    db: &mut Db,
    customer_id: i64,
    qty: f64,
    payment_type: &str,
    date: &str,
) -> realinvoice_core::Invoice {
    let cement = item(db, "CEM-OPC-53"); // 410.00 at 28%
    db.create_invoice(&NewInvoice {
        customer_id,
        date: Some(date.to_string()),
        payment_type: payment_type.into(),
        created_by_user_id: None,
        invoice_discount_type: DiscountType::None,
        invoice_discount_value: 0.0,
        lines: vec![NewInvoiceLine {
            item_id: cement.id,
            qty,
            rate: None,
            tax_rate: None,
            discount_type: DiscountType::None,
            discount_value: 0.0,
        }],
    })
    .unwrap()
}

/// Paying at the counter is now a fact with a receipt behind it, not a label.
#[test]
fn a_cash_sale_is_recorded_as_paid_with_a_matching_payment() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");

    let invoice = bill(&mut db, buyer.id, 10.0, "cash", "2026-09-10");
    assert_eq!(invoice.grand_total, 5_248.00);
    assert_eq!(invoice.amount_paid, 5_248.00);
    assert_eq!(invoice.amount_due, 0.0);
    assert_eq!(invoice.payment_status, PaymentStatus::Paid);

    // A receipt exists for it, for the amount and by the method that was chosen.
    let payments = db.payments_for_invoice(invoice.id).unwrap();
    assert_eq!(payments.len(), 1);
    assert_eq!(payments[0].amount, 5_248.00);
    assert_eq!(payments[0].payment_method, "cash");
    assert_eq!(payments[0].invoice_id, Some(invoice.id));

    assert_eq!(db.customer_balance(buyer.id).unwrap(), 0.0, "nothing owed");
    assert!(db.open_invoices(buyer.id).unwrap().is_empty());
}

/// The whole point of the stage: "put it on my account".
#[test]
fn a_credit_sale_is_unpaid_and_owed_in_full() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");

    let invoice = bill(&mut db, buyer.id, 10.0, "credit", "2026-09-10");
    assert_eq!(invoice.amount_paid, 0.0);
    assert_eq!(invoice.amount_due, 5_248.00);
    assert_eq!(invoice.payment_status, PaymentStatus::Unpaid);

    assert!(db.payments_for_invoice(invoice.id).unwrap().is_empty(), "no receipt was issued");
    assert_eq!(db.customer_balance(buyer.id).unwrap(), 5_248.00);
    assert_eq!(db.open_invoices(buyer.id).unwrap().len(), 1);
}

/// Part now, the rest later — the sequence the verification walks through.
#[test]
fn partial_payments_move_the_status_and_then_clear_it() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let invoice = bill(&mut db, buyer.id, 10.0, "credit", "2026-09-10");

    let first = db
        .record_payment(&NewPayment {
            customer_id: buyer.id,
            invoice_id: Some(invoice.id),
            amount: 2_000.0,
            payment_method: "cash".into(),
            date: Some("2026-09-12".into()),
            notes: Some("Part payment".into()),
            created_by_user_id: None,
        })
        .unwrap();
    assert_eq!(first.applied.as_ref().unwrap().amount, 2_000.00);
    assert!(first.on_account.is_none(), "it all fitted on the invoice");
    assert_eq!(first.balance, 3_248.00);

    let after_first = db.get_invoice(invoice.id).unwrap().unwrap();
    assert_eq!(after_first.amount_paid, 2_000.00);
    assert_eq!(after_first.amount_due, 3_248.00);
    assert_eq!(after_first.payment_status, PaymentStatus::PartiallyPaid);

    let second = db
        .record_payment(&NewPayment {
            customer_id: buyer.id,
            invoice_id: Some(invoice.id),
            amount: 3_248.0,
            payment_method: "upi".into(),
            date: Some("2026-09-20".into()),
            notes: None,
            created_by_user_id: None,
        })
        .unwrap();
    assert_eq!(second.balance, 0.0);

    let cleared = db.get_invoice(invoice.id).unwrap().unwrap();
    assert_eq!(cleared.amount_paid, 5_248.00);
    assert_eq!(cleared.amount_due, 0.0);
    assert_eq!(cleared.payment_status, PaymentStatus::Paid);
    assert!(db.open_invoices(buyer.id).unwrap().is_empty());
}

/// Money handed over is never refused and never lost.
#[test]
fn a_payment_bigger_than_the_invoice_leaves_the_rest_on_account() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let invoice = bill(&mut db, buyer.id, 10.0, "credit", "2026-09-10"); // 5,248.00

    let recorded = db
        .record_payment(&NewPayment {
            customer_id: buyer.id,
            invoice_id: Some(invoice.id),
            amount: 8_000.0,
            payment_method: "cash".into(),
            date: None,
            notes: None,
            created_by_user_id: None,
        })
        .unwrap();

    // Split in two: what the bill could absorb, and the rest.
    assert_eq!(recorded.applied.as_ref().unwrap().amount, 5_248.00);
    assert_eq!(recorded.applied.as_ref().unwrap().invoice_id, Some(invoice.id));
    assert_eq!(recorded.on_account.as_ref().unwrap().amount, 2_752.00);
    assert_eq!(recorded.on_account.as_ref().unwrap().invoice_id, None);

    assert_eq!(db.get_invoice(invoice.id).unwrap().unwrap().payment_status, PaymentStatus::Paid);
    // The shop is now holding 2,752.00 of theirs.
    assert_eq!(recorded.balance, -2_752.00);
    assert_eq!(db.customer_balance(buyer.id).unwrap(), -2_752.00);

    // And that credit settles the next bill's worth of debt on its own.
    bill(&mut db, buyer.id, 5.0, "credit", "2026-09-15"); // 2,624.00
    assert_eq!(db.customer_balance(buyer.id).unwrap(), -128.00);
}

/// A payment against no particular bill, from somebody clearing a running balance.
#[test]
fn an_account_payment_reduces_the_balance_without_touching_any_invoice() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let first = bill(&mut db, buyer.id, 10.0, "credit", "2026-09-10"); // 5,248.00
    let second = bill(&mut db, buyer.id, 5.0, "credit", "2026-09-11"); // 2,624.00
    assert_eq!(db.customer_balance(buyer.id).unwrap(), 7_872.00);

    let recorded = db
        .record_payment(&NewPayment {
            customer_id: buyer.id,
            invoice_id: None,
            amount: 3_000.0,
            payment_method: "bank transfer".into(),
            date: Some("2026-09-14".into()),
            notes: Some("Towards the account".into()),
            created_by_user_id: None,
        })
        .unwrap();

    assert!(recorded.applied.is_none());
    assert_eq!(recorded.on_account.as_ref().unwrap().amount, 3_000.00);
    assert_eq!(recorded.balance, 4_872.00);

    // Neither invoice moved: the money was not put against either of them.
    assert_eq!(db.get_invoice(first.id).unwrap().unwrap().amount_due, 5_248.00);
    assert_eq!(db.get_invoice(second.id).unwrap().unwrap().amount_due, 2_624.00);
    assert_eq!(db.open_invoices(buyer.id).unwrap().len(), 2);
}

/// A credit note reduces what is owed exactly as a payment does, and is counted apart.
#[test]
fn a_credit_note_reduces_what_is_owed_without_being_a_payment() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let invoice = bill(&mut db, buyer.id, 10.0, "credit", "2026-09-10"); // 5,248.00

    let line = db.creditable_lines(invoice.id).unwrap().remove(0);
    db.create_credit_note(&NewCreditNote {
        original_invoice_id: invoice.id,
        date: Some("2026-09-13".into()),
        reason: "Two bags returned".into(),
        created_by_user_id: None,
        lines: vec![NewCreditNoteLine { invoice_line_id: line.invoice_line_id, qty: 2.0 }],
    })
    .unwrap();

    // 2 x 410 = 820 plus 28% = 1,049.60 off what is owed.
    let after = db.get_invoice(invoice.id).unwrap().unwrap();
    assert_eq!(after.amount_due, 4_198.40);
    assert_eq!(after.amount_paid, 0.0, "a credit note is not money received");
    assert_eq!(after.payment_status, PaymentStatus::PartiallyPaid);
    assert_eq!(db.customer_balance(buyer.id).unwrap(), 4_198.40);

    let ledger = db.customer_ledger(buyer.id).unwrap();
    assert_eq!(ledger.credited_total, 1_049.60);
    assert_eq!(ledger.paid_total, 0.0);
}

/// The passbook: everything in date order with the balance after each line.
#[test]
fn the_ledger_interleaves_invoices_payments_and_credit_notes_by_date() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");

    let first = bill(&mut db, buyer.id, 10.0, "credit", "2026-09-10"); // 5,248.00
    db.record_payment(&NewPayment {
        customer_id: buyer.id,
        invoice_id: Some(first.id),
        amount: 2_000.0,
        payment_method: "cash".into(),
        date: Some("2026-09-12".into()),
        notes: None,
        created_by_user_id: None,
    })
    .unwrap();

    let second = bill(&mut db, buyer.id, 5.0, "credit", "2026-09-14"); // 2,624.00
    let line = db.creditable_lines(second.id).unwrap().remove(0);
    db.create_credit_note(&NewCreditNote {
        original_invoice_id: second.id,
        date: Some("2026-09-16".into()),
        reason: "One bag short".into(),
        created_by_user_id: None,
        lines: vec![NewCreditNoteLine { invoice_line_id: line.invoice_line_id, qty: 1.0 }],
    })
    .unwrap();

    let ledger = db.customer_ledger(buyer.id).unwrap();
    let shape: Vec<(&str, f64, f64)> = ledger
        .entries
        .iter()
        .map(|e| {
            let kind = match e.kind {
                LedgerEntryKind::Invoice => "invoice",
                LedgerEntryKind::Payment => "payment",
                LedgerEntryKind::CreditNote => "credit note",
            };
            (kind, e.change, e.balance)
        })
        .collect();

    assert_eq!(
        shape,
        vec![
            ("invoice", 5_248.00, 5_248.00),
            ("payment", -2_000.00, 3_248.00),
            ("invoice", 2_624.00, 5_872.00),
            ("credit note", -524.80, 5_347.20),
        ]
    );

    // The running balance ends where the balance query says it does.
    assert_eq!(ledger.entries.last().unwrap().balance, ledger.balance);
    assert_eq!(ledger.balance, 5_347.20);
    assert_eq!(ledger.billed_total, 7_872.00);
    assert_eq!(ledger.paid_total, 2_000.00);
    assert_eq!(ledger.credited_total, 524.80);
    assert_eq!(ledger.open_invoices.len(), 2);
}

/// A bill raised and settled on the same day reads the way it happened.
#[test]
fn same_day_entries_show_the_invoice_before_what_came_off_it() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");

    // A cash sale writes both rows with today's date.
    let invoice = bill(&mut db, buyer.id, 10.0, "cash", "2026-09-10");
    let ledger = db.customer_ledger(buyer.id).unwrap();

    assert_eq!(ledger.entries.len(), 2);
    assert_eq!(ledger.entries[0].kind, LedgerEntryKind::Invoice);
    assert_eq!(ledger.entries[0].balance, 5_248.00, "billed first");
    assert_eq!(ledger.entries[1].kind, LedgerEntryKind::Payment);
    assert_eq!(ledger.entries[1].balance, 0.0, "then settled");
    assert_eq!(ledger.entries[0].reference, invoice.invoice_no);
}

/// The view a shop owner actually opens.
#[test]
fn the_customers_list_shows_and_sorts_by_what_is_owed() {
    let mut db = seeded_db();
    let balaji = customer(&db, "9840012345");
    let kaveri = customer(&db, "9791045678");

    bill(&mut db, balaji.id, 10.0, "credit", "2026-09-10"); // 5,248.00 owed
    bill(&mut db, kaveri.id, 30.0, "credit", "2026-09-10"); // 15,744.00 owed
    bill(&mut db, balaji.id, 5.0, "cash", "2026-09-11"); // settled, owes nothing more

    let by_owed = db
        .list_customers(&CustomerFilter {
            sort: CustomerSort::OutstandingDesc,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(by_owed[0].customer.id, kaveri.id);
    assert_eq!(by_owed[0].outstanding, 15_744.00);
    assert_eq!(by_owed[1].customer.id, balaji.id);
    assert_eq!(by_owed[1].outstanding, 5_248.00);
    assert_eq!(by_owed[2].outstanding, 0.0, "and everyone else owes nothing");

    // The figure matches what that customer's own ledger says, which is the property
    // that stops the list and the detail page telling two different stories.
    for summary in &by_owed {
        let ledger = db.customer_ledger(summary.customer.id).unwrap();
        assert_eq!(summary.outstanding, ledger.balance, "{}", summary.customer.name);
    }

    // Only the two who owe something.
    let owing =
        db.list_customers(&CustomerFilter { owing_only: true, ..Default::default() }).unwrap();
    assert_eq!(owing.len(), 2);

    // Ascending puts them the other way up.
    let ascending = db
        .list_customers(&CustomerFilter {
            sort: CustomerSort::OutstandingAsc,
            owing_only: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(ascending[0].customer.id, balaji.id);

    // And searching narrows it.
    let found = db
        .list_customers(&CustomerFilter { text: Some("kaveri".into()), ..Default::default() })
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].customer.id, kaveri.id);
    assert_eq!(found[0].invoice_count, 1);
    assert_eq!(found[0].last_billed.as_deref(), Some("2026-09-10"));
}

#[test]
fn a_credit_limit_is_checked_against_what_is_already_owed() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");

    // Nobody has set a limit, so nothing is over it.
    let unlimited = db.check_credit_limit(buyer.id, 1_000_000.0).unwrap();
    assert_eq!(unlimited.credit_limit, None);
    assert!(!unlimited.over_limit, "no limit set is not a limit of zero");

    db.set_credit_limit(buyer.id, Some(10_000.0)).unwrap();
    assert_eq!(db.get_customer(buyer.id).unwrap().unwrap().credit_limit, Some(10_000.0));

    bill(&mut db, buyer.id, 10.0, "credit", "2026-09-10"); // 5,248.00 owed

    let inside = db.check_credit_limit(buyer.id, 4_000.0).unwrap();
    assert_eq!(inside.balance, 5_248.00);
    assert!(!inside.over_limit, "9,248 is inside 10,000");

    let outside = db.check_credit_limit(buyer.id, 5_000.0).unwrap();
    assert!(outside.over_limit, "10,248 is not");

    // Paying some off makes room again.
    db.record_payment(&NewPayment {
        customer_id: buyer.id,
        invoice_id: None,
        amount: 3_000.0,
        payment_method: "cash".into(),
        date: None,
        notes: None,
        created_by_user_id: None,
    })
    .unwrap();
    assert!(!db.check_credit_limit(buyer.id, 5_000.0).unwrap().over_limit);

    // Clearing the limit removes the ceiling entirely.
    db.set_credit_limit(buyer.id, None).unwrap();
    assert!(!db.check_credit_limit(buyer.id, 999_999.0).unwrap().over_limit);
    assert!(db.set_credit_limit(buyer.id, Some(-1.0)).is_err());
}

#[test]
fn a_payment_is_refused_when_it_makes_no_sense() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let other = customer(&db, "9791045678");
    let invoice = bill(&mut db, buyer.id, 10.0, "credit", "2026-09-10");

    let mut bad = |p: NewPayment| db.record_payment(&p).is_err();

    let base = NewPayment {
        customer_id: buyer.id,
        invoice_id: None,
        amount: 100.0,
        payment_method: "cash".into(),
        date: None,
        notes: None,
        created_by_user_id: None,
    };

    assert!(bad(NewPayment { amount: 0.0, ..base.clone() }), "zero is not a payment");
    assert!(bad(NewPayment { amount: -50.0, ..base.clone() }), "nor is a negative one");
    assert!(bad(NewPayment { payment_method: "  ".into(), ..base.clone() }));
    assert!(bad(NewPayment { date: Some("not-a-date".into()), ..base.clone() }));
    assert!(bad(NewPayment { customer_id: 9_999, ..base.clone() }));
    assert!(bad(NewPayment { invoice_id: Some(9_999), ..base.clone() }));

    // And a payment aimed at somebody else's invoice.
    assert!(bad(NewPayment {
        customer_id: other.id,
        invoice_id: Some(invoice.id),
        ..base.clone()
    }));

    // None of that wrote anything.
    assert!(db.payments_for_customer(buyer.id).unwrap().is_empty());
    assert_eq!(db.customer_balance(buyer.id).unwrap(), 5_248.00);
}

/// Payments are business data, so the back office hears about them.
#[test]
fn payments_are_queued_for_sync_and_re_queue_their_invoice() {
    let mut db = seeded_db();
    let buyer = customer(&db, "9840012345");
    let invoice = bill(&mut db, buyer.id, 10.0, "credit", "2026-09-10");

    // A credit sale queues its invoice and lines, and no payment.
    assert_eq!(db.pending_sync_rows_for("payments").unwrap().len(), 0);
    assert_eq!(db.pending_sync_rows_for("invoices").unwrap().len(), 1);

    db.record_payment(&NewPayment {
        customer_id: buyer.id,
        invoice_id: Some(invoice.id),
        amount: 2_000.0,
        payment_method: "cash".into(),
        date: None,
        notes: None,
        created_by_user_id: None,
    })
    .unwrap();

    let queued = db.pending_sync_rows_for("payments").unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].op, "insert");

    // The invoice went out again carrying its new balance, rather than leaving the far
    // end holding a row that says the bill is still unpaid in full.
    let invoice_rows = db.pending_sync_rows_for("invoices").unwrap();
    assert_eq!(invoice_rows.len(), 2);
    assert_eq!(invoice_rows[1].op, "update");
    assert!(
        invoice_rows[1].payload_json.contains("partially_paid"),
        "{}",
        invoice_rows[1].payload_json
    );
}

/// Everything billed before the ledger existed is settled, not a sudden pile of debt.
#[test]
fn invoices_from_before_the_ledger_are_treated_as_paid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pre-ledger.sqlite");

    // A database at the previous stage's schema, with one invoice in it.
    {
        let mut conn = rusqlite::Connection::open(&path).unwrap();
        realinvoice_core::schema::migrate_through(&mut conn, "008_discounts").unwrap();
        conn.execute(
            "INSERT INTO customers (name, gstin, place_of_supply, mobile)
             VALUES ('Old Buyer', NULL, 'TN', '9000000001')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invoices
                 (invoice_no, date, customer_id, subtotal, cgst, sgst, igst, grand_total,
                  payment_type, sync_status, created_at, pre_discount_subtotal)
             VALUES ('RI-2025-0001', '2025-04-01', 1, 1000.0, 90.0, 90.0, 0.0, 1180.0,
                     'cash', 'synced', '2025-04-01 10:00:00', 1000.0)",
            [],
        )
        .unwrap();
    }

    // Opening it runs 009 and everything settles.
    let db = Db::open(&path).unwrap();
    let invoice = db.get_invoice(1).unwrap().unwrap();
    assert_eq!(invoice.grand_total, 1_180.00);
    assert_eq!(invoice.amount_paid, 1_180.00);
    assert_eq!(invoice.amount_due, 0.0);
    assert_eq!(invoice.payment_status, PaymentStatus::Paid);
    assert_eq!(db.customer_balance(1).unwrap(), 0.0);

    // And no receipt was invented for it: the migration marks it settled without
    // pretending a payment was taken that nobody recorded.
    assert!(db.payments_for_invoice(1).unwrap().is_empty());
}
