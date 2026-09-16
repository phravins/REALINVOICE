//! Demo data, so a fresh install has something to search for.
//!
//! Called by the Office Console the first time it creates its database, and by tests
//! that need a customer and a couple of items to exist.

use crate::db::Db;
use crate::error::Result;
use crate::models::{ItemPrice, NewCustomer, NewItem};

/// Customers a fresh database starts with.
pub fn demo_customers() -> Vec<NewCustomer> {
    vec![
        NewCustomer {
            name: "Sri Balaji Traders".into(),
            gstin: Some("33AABCS1429B1ZP".into()),
            place_of_supply: "TN".into(),
            mobile: "9840012345".into(),
            price_list_id: None,
        },
        NewCustomer {
            name: "Kaveri Hardware".into(),
            gstin: Some("33AAGCK9021P1Z4".into()),
            place_of_supply: "TN".into(),
            mobile: "9791045678".into(),
            price_list_id: None,
        },
        // Inter-state, so the IGST path is reachable from the UI too.
        NewCustomer {
            name: "Deccan Supplies".into(),
            gstin: Some("29AACCD4455K1ZR".into()),
            place_of_supply: "KA".into(),
            mobile: "9845567890".into(),
            price_list_id: None,
        },
        NewCustomer {
            name: "Ishta Capital Investments".into(),
            gstin: Some("33AAAAA0000A1Z1".into()),
            place_of_supply: "TN".into(),
            mobile: "9600011223".into(),
            price_list_id: None,
        },
        // Unregistered walk-in: no GSTIN.
        NewCustomer {
            name: "Walk-in Customer".into(),
            gstin: None,
            place_of_supply: "TN".into(),
            mobile: "9000000000".into(),
            price_list_id: None,
        },
    ]
}

/// Items a fresh database starts with.
pub fn demo_items() -> Vec<NewItem> {
    vec![
        // The two the worked example bills: 1 x 45,000 + 5 x 12,000 = 1,05,000 @ 18%.
        NewItem {
            item_code: "RACK-42U-PRO".into(),
            description: "42U Server Rack Pro".into(),
            rate: 45_000.0,
            tax_rate: 18.0,
            uom: "NOS".into(),
            custom: false,
        },
        NewItem {
            item_code: "ABCOS-ENT-LIC".into(),
            description: "aBCOS Enterprise Lic".into(),
            rate: 12_000.0,
            tax_rate: 18.0,
            uom: "LIC".into(),
            custom: false,
        },
        NewItem {
            item_code: "CEM-OPC-53".into(),
            description: "OPC 53 Grade Cement".into(),
            rate: 410.0,
            tax_rate: 28.0,
            uom: "BAG".into(),
            custom: false,
        },
        NewItem {
            item_code: "TMT-12MM".into(),
            description: "TMT Steel Bar 12mm".into(),
            rate: 620.0,
            tax_rate: 18.0,
            uom: "ROD".into(),
            custom: false,
        },
        NewItem {
            item_code: "PVC-PIPE-4".into(),
            description: "PVC Pipe 4 inch".into(),
            rate: 285.5,
            tax_rate: 18.0,
            uom: "NOS".into(),
            custom: false,
        },
        NewItem {
            item_code: "PAINT-WH-20".into(),
            description: "Emulsion Paint White 20L".into(),
            rate: 3150.0,
            tax_rate: 18.0,
            uom: "CAN".into(),
            custom: false,
        },
        NewItem {
            item_code: "SAND-M-UNIT".into(),
            description: "M-Sand per unit".into(),
            rate: 4800.0,
            tax_rate: 5.0,
            uom: "UNIT".into(),
            custom: false,
        },
    ]
}

/// Writes the demo customers and items into `db`. Idempotent — both go through the
/// upsert paths, keyed on mobile and item code.
pub fn seed_demo_data(db: &mut Db) -> Result<()> {
    for customer in demo_customers() {
        db.upsert_customer(&customer)?;
    }
    // Priced on the default list as well as carrying a base rate, so a fresh install and
    // a database that came through the price-list migration look identical — both state
    // the default list's rates rather than one stating them and the other falling back.
    let default_list = db.default_price_list()?.id;
    for item in demo_items() {
        let saved = db.upsert_item(&item)?;
        db.set_item_prices(
            saved.id,
            &[ItemPrice { item_id: saved.id, price_list_id: default_list, rate: saved.rate }],
        )?;
    }
    Ok(())
}

/// Seeds only if the database has no customers yet, so a real installation's data is
/// never touched on restart.
pub fn seed_if_empty(db: &mut Db) -> Result<bool> {
    if db.search_customer("9840012345")?.is_some() || !db.search_item("")?.is_empty() {
        return Ok(false);
    }
    seed_demo_data(db)?;
    Ok(true)
}
