# Office Console — Stage 1

The native shell for the billing counter, wired to `realinvoice-core` and proving the
`invoke()` bridge. No transaction UI yet: that is stage 2.

> **Superseded in part.** The Billing pane described here — a mobile search box and a
> List Today's Invoices button — was scaffolding, and stage 2 replaced it with the NEW
> TRANSACTION card. See [office-console-stage-2.md](office-console-stage-2.md). Everything
> else on this page (the shell, the command layer, the database location) still holds.

## What this stage delivers

- A Tauri v2 app under `desktop/src-tauri` depending on `core` as a path dependency.
- Six commands, each a thin wrapper over core's `Db` — no billing logic duplicated.
- A plain HTML/CSS/JS frontend (three files, no framework, no build step) with the title
  bar, the tab row, and a Billing pane that exercises two of those commands.
- The console's own `db.sqlite` in Tauri's app config dir, migrated and seeded on startup.

## Commands

| Command | Signature | Core call |
| --- | --- | --- |
| `node_status` | `() -> NodeStatus` | — (node name + stubbed `connected`) |
| `search_customer` | `(mobile: String) -> Option<Customer>` | `Db::search_customer` |
| `search_item` | `(query: String) -> Vec<Item>` | `Db::search_item` |
| `create_invoice` | `(payload: NewInvoicePayload) -> Invoice` | `Db::create_invoice` |
| `add_line_item` | `(invoiceId: i64, line: NewLinePayload) -> InvoiceLine` | `Db::add_line_item` |
| `list_todays_invoices` | `() -> Vec<Invoice>` | `Db::list_todays_invoices` |

Fallible commands return `Result<T, String>`; the frontend renders the string. `Option`
comes back as `null`, and an empty `Vec` as `[]` — a miss is not an error.

## Database location

`AppState` opens one connection at startup and every command reuses it under a `Mutex`.
The file lives in Tauri's app config dir, keyed on the bundle identifier
`in.osworks.realinvoice.desktop`:

| OS | Path |
| --- | --- |
| Linux | `~/.config/in.osworks.realinvoice.desktop/db.sqlite` |
| macOS | `~/Library/Application Support/in.osworks.realinvoice.desktop/db.sqlite` |
| Windows | `%APPDATA%\in.osworks.realinvoice.desktop\db.sqlite` |

This is the console's own file by design — the counter has to keep billing when every
other machine is unreachable. On first run migrations apply and
`realinvoice_core::seed::seed_if_empty` loads four demo customers and five demo items;
on later runs it sees existing data and leaves it alone.

## Running it

Linux needs the usual Tauri v2 system dependencies:

```sh
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
    libayatana-appindicator3-dev librsvg2-dev patchelf libxdo-dev libssl-dev
```

Then:

```sh
cargo test --workspace
cargo run -p realinvoice-desktop       # or: cd desktop/src-tauri && cargo tauri dev
```

There is no dev server. `frontendDist` points straight at `desktop/frontend`, so the
three static files are what ships — editing them and restarting is the whole loop.

To put data in a node's database by hand:

```sh
cargo run -p realinvoice-core --example seed_db -- \
    ~/.config/in.osworks.realinvoice.desktop/db.sqlite --invoice
```

## Verified

Run against a fresh config dir under Xvfb, driven with `xdotool`:

1. The window opens titled `REALINVOICE DESKTOP [Node: POS-01]` with a green
   `[Connected]` badge, filled in from `node_status` — so `invoke()` works before any
   button is touched. The status bar shows the resolved db path.
2. Mobile `9840012345` → **Sri Balaji Traders**, GSTIN `33AABCS1429B1ZP`, place of supply
   `TN`. Mobile `9999999999` → "No customer found", not an error.
3. **List Today's Invoices** on the fresh database renders "No invoices raised today."
4. After `seed_db --invoice` wrote `RI-2026-0001` into that same file, the same button
   rendered `RI-2026-0001 · 2026-09-11 · cash · ₹8,906.00 · pending`.
5. Clicking Inventory, Analytics, Sync or Settings swaps to a "Coming soon" pane.
6. Reopened with `sqlite3`'s Python bindings from a separate process, the file held the
   invoice, both `invoice_lines`, and three unsynced `sync_queue` rows (one `invoices`
   insert, two `invoice_lines` inserts) — plus the queued seed writes.

## Deliberately not in this stage

- The transaction UI: item rows, line totals, the GST summary panel, Print & Lock (F5).
- Real connectivity behind the `[Connected]` badge — it is hardcoded `true` in
  `commands::node_status` and is the sync stage's job.
- Anything that drains `sync_queue`. Rows are queued and left alone; nothing networks.
- The Inventory, Analytics, Sync and Settings panes.
