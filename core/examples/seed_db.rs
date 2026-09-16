//! Seeds a RealInvoice database and, optionally, raises a demo invoice against it.
//!
//! Handy for bringing a node up with something to look at, and for verifying a running
//! Office Console against its own file:
//!
//! ```sh
//! cargo run -p realinvoice-core --example seed_db -- ~/.config/in.osworks.realinvoice.desktop/db.sqlite --invoice
//! ```

use realinvoice_core::{seed, Db, DiscountType, NewInvoice, NewInvoiceLine};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "db.sqlite".to_string());
    let with_invoice = args.any(|a| a == "--invoice");

    let mut db = Db::open(&path)?;
    seed::seed_demo_data(&mut db)?;
    println!("seeded demo customers and items into {path}");

    if with_invoice {
        let buyer = db.search_customer("9840012345")?.ok_or("demo customer 9840012345 missing")?;
        let cement = db.search_item("CEM-OPC-53")?.into_iter().next().ok_or("item missing")?;
        let steel = db.search_item("TMT-12MM")?.into_iter().next().ok_or("item missing")?;

        let invoice = db.create_invoice(&NewInvoice {
            customer_id: buyer.id,
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
                    qty: 5.0,
                    rate: None,
                    tax_rate: None,
                    discount_type: DiscountType::None,
                    discount_value: 0.0,
                },
            ],
            invoice_discount_type: DiscountType::None,
            invoice_discount_value: 0.0,
        })?;

        println!(
            "raised {} for {} — subtotal {:.2}, CGST {:.2}, SGST {:.2}, IGST {:.2}, total {:.2}",
            invoice.invoice_no,
            buyer.name,
            invoice.subtotal,
            invoice.cgst,
            invoice.sgst,
            invoice.igst,
            invoice.grand_total
        );
        println!(
            "{} line(s), {} row(s) waiting in sync_queue",
            db.invoice_lines(invoice.id)?.len(),
            db.pending_sync_rows()?.len()
        );
    }

    Ok(())
}
