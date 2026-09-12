use std::process::Command;

fn main() {
    // Stamp the build date so the About pane can show it without a runtime lookup.
    // `SOURCE_DATE_EPOCH` is honoured when set, so reproducible builds stay reproducible.
    let date = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|epoch| epoch.parse::<i64>().ok())
        .and_then(|epoch| {
            Command::new("date").args(["-u", "-d", &format!("@{epoch}"), "+%Y-%m-%d"]).output().ok()
        })
        .or_else(|| Command::new("date").args(["-u", "+%Y-%m-%d"]).output().ok())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=REALINVOICE_BUILD_DATE={date}");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    tauri_build::build()
}
