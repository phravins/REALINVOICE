// Keep the console window off Windows release builds; the app is the UI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    realinvoice_desktop_lib::run()
}
