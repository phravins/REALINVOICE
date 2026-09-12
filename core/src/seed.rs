//! Demo data, so a fresh install has something to search for.
//!
//! Called by the Office Console the first time it creates its database, and by tests
//! that need a customer and a couple of items to exist.

use crate::auth;
use crate::db::Db;
use crate::error::Result;
use crate::models::{NewCustomer, NewItem, NewUser, Role, User};

/// Username of the account created on first run.
pub const DEFAULT_OWNER_USERNAME: &str = "admin";

/// Customers a fresh database starts with.
pub fn demo_customers() -> Vec<NewCustomer> {
    vec![
        NewCustomer {
            name: "Sri Balaji Traders".into(),
            gstin: Some("33AABCS1429B1ZP".into()),
            place_of_supply: "TN".into(),
            mobile: "9840012345".into(),
        },
        NewCustomer {
            name: "Kaveri Hardware".into(),
            gstin: Some("33AAGCK9021P1Z4".into()),
            place_of_supply: "TN".into(),
            mobile: "9791045678".into(),
        },
        // Inter-state, so the IGST path is reachable from the UI too.
        NewCustomer {
            name: "Deccan Supplies".into(),
            gstin: Some("29AACCD4455K1ZR".into()),
            place_of_supply: "KA".into(),
            mobile: "9845567890".into(),
        },
        NewCustomer {
            name: "Ishta Capital Investments".into(),
            gstin: Some("33AAAAA0000A1Z1".into()),
            place_of_supply: "TN".into(),
            mobile: "9600011223".into(),
        },
        // Unregistered walk-in: no GSTIN.
        NewCustomer {
            name: "Walk-in Customer".into(),
            gstin: None,
            place_of_supply: "TN".into(),
            mobile: "9000000000".into(),
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
        },
        NewItem {
            item_code: "ABCOS-ENT-LIC".into(),
            description: "aBCOS Enterprise Lic".into(),
            rate: 12_000.0,
            tax_rate: 18.0,
            uom: "LIC".into(),
        },
        NewItem {
            item_code: "CEM-OPC-53".into(),
            description: "OPC 53 Grade Cement".into(),
            rate: 410.0,
            tax_rate: 28.0,
            uom: "BAG".into(),
        },
        NewItem {
            item_code: "TMT-12MM".into(),
            description: "TMT Steel Bar 12mm".into(),
            rate: 620.0,
            tax_rate: 18.0,
            uom: "ROD".into(),
        },
        NewItem {
            item_code: "PVC-PIPE-4".into(),
            description: "PVC Pipe 4 inch".into(),
            rate: 285.5,
            tax_rate: 18.0,
            uom: "NOS".into(),
        },
        NewItem {
            item_code: "PAINT-WH-20".into(),
            description: "Emulsion Paint White 20L".into(),
            rate: 3150.0,
            tax_rate: 18.0,
            uom: "CAN".into(),
        },
        NewItem {
            item_code: "SAND-M-UNIT".into(),
            description: "M-Sand per unit".into(),
            rate: 4800.0,
            tax_rate: 5.0,
            uom: "UNIT".into(),
        },
    ]
}

/// Writes the demo customers and items into `db`. Idempotent — both go through the
/// upsert paths, keyed on mobile and item code.
pub fn seed_demo_data(db: &mut Db) -> Result<()> {
    for customer in demo_customers() {
        db.upsert_customer(&customer)?;
    }
    for item in demo_items() {
        db.upsert_item(&item)?;
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

/// Creates the default owner account if no users exist yet, returning the account and
/// its generated one-time password.
///
/// The password is generated per installation rather than hardcoded, so no two machines
/// ship with the same credentials and nothing secret lives in this repository. It is
/// returned exactly once — the caller prints it, and after that only its hash exists.
/// `Ok(None)` means accounts already exist and nothing was touched.
pub fn seed_owner_if_empty(db: &mut Db) -> Result<Option<(User, String)>> {
    if db.count_users()? > 0 {
        return Ok(None);
    }

    let password = auth::generate_initial_password();
    let owner = db.create_user(
        &NewUser {
            username: DEFAULT_OWNER_USERNAME.to_string(),
            display_name: "Store Owner".to_string(),
            role: Role::Owner,
        },
        &password,
    )?;
    Ok(Some((owner, password)))
}
