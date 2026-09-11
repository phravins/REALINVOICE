//! RealInvoice Office Console — the Tauri shell.
//!
//! Stage 1 wires the native shell to `realinvoice-core` and proves the `invoke()` path
//! end to end: a native window, a tab row, and a Billing pane that can resolve a customer
//! and list the day's invoices out of this node's own SQLite file. The full transaction
//! screen is stage 2.

pub mod commands;
pub mod state;

use tauri::Manager;

pub use state::{AppState, DB_FILE_NAME, NODE_NAME};

/// Boots the Office Console.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let state = AppState::new(dir.join(DB_FILE_NAME))?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::node_status,
            commands::search_customer,
            commands::search_item,
            commands::create_invoice,
            commands::add_line_item,
            commands::list_todays_invoices,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the RealInvoice Office Console");
}
