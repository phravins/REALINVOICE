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

            // The push worker runs on Tauri's own runtime, which is where the commands
            // run too. It never blocks startup: with no endpoint configured — or with one
            // that is unreachable — it simply idles or retries, and billing is unaffected
            // either way.
            let config = state.sync_config().map_err(std::io::Error::other)?;
            let (handle, worker) = realinvoice_core::sync::start(state.db_arc(), config);
            state.set_sync(handle);
            tauri::async_runtime::spawn(worker);

            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Reachable without a session; everything below it requires one.
            commands::auth_status,
            commands::create_first_user,
            commands::login,
            commands::logout,
            commands::get_theme,
            commands::set_theme,
            commands::node_status,
            commands::app_info,
            commands::list_users,
            commands::create_user,
            commands::search_customer,
            commands::create_customer,
            commands::search_item,
            commands::list_items,
            commands::count_items,
            commands::save_item,
            commands::create_custom_item,
            commands::clear_demo_data,
            commands::price_lists,
            commands::price_list_for_customer,
            commands::default_price_list,
            commands::create_price_list,
            commands::rename_price_list,
            commands::set_default_price_list,
            commands::set_customer_price_list,
            commands::item_prices,
            commands::set_item_prices,
            commands::resolve_rates,
            commands::analytics,
            commands::sync_status,
            commands::sync_now,
            commands::set_sync_endpoint,
            commands::set_sync_token,
            commands::create_invoice,
            commands::add_line_item,
            commands::list_todays_invoices,
            commands::list_invoices,
            commands::invoice_detail,
            commands::creditable_lines,
            commands::create_credit_note,
            commands::credit_history,
            commands::quote_invoice,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the RealInvoice Office Console");
}
