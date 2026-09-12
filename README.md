# RealInvoice

Offline-first GST invoicing for Indian SMEs, built as a monorepo around one shared Rust
core and three runtimes that each serve a different desk in the business. The **Office
Console** is a Tauri desktop app — the billing counter's native screen, running against a
local SQLite file so invoicing never stops when the internet does; the **Back-Office Web**
is a Phoenix LiveView application for owners and accountants who want reporting and
multi-node oversight from a browser; and the **Logistics Desk** is a Ratatui terminal UI
for the dispatch and stores team, who live on keyboard-driven, low-bandwidth machines. All
three read and write the same schema defined by the `core` crate, and every local write is
queued into a `sync_queue` table so nodes can reconcile with each other later. Only the
Office Console and the core crate exist today — the Phoenix and Ratatui runtimes land in
stages 2 and 3.

## Layout

```
realinvoice/
  core/              Rust lib crate: realinvoice-core (schema, GST, numbering, Db)
  desktop/           Office Console
    src-tauri/       Tauri shell + commands (depends on core)
    frontend/        plain HTML + JS UI, no framework
  docs/              design notes and stage plans
  Cargo.toml         workspace root
```

## Status

| Stage | Component | State |
| --- | --- | --- |
| 1 | `core` — schema, GST calc, invoice numbering, `Db` | done |
| 1 | Office Console — native shell, Tauri commands, stub UI | done |
| 2 | Office Console — NEW TRANSACTION card, live GST summary | done |
| 2 | Back-Office Web (Phoenix LiveView) | not started |
| 3 | Office Console — Print & Lock saves, locks and resets | done |
| 3 | Logistics Desk (Ratatui TUI) | not started |
| 4 | Office Console — invoice history, detail view, reprint | done |
| 5 | Office Console — sign-in, payment type, shortcuts | done |
| 6 | Office Console — layout system and UI pass | done |
| 7 | Office Console — design tokens, light/dark themes, branding, About | done |
| 8 | Office Console — flat shell, sidebar, unboxed surfaces | done |
| — | Sync worker that drains `sync_queue` | not started |

## Build

```sh
cargo test -p realinvoice-core     # core: unit + integration tests
cargo build                        # whole workspace
cd desktop/src-tauri && cargo tauri dev
```

The desktop app needs the usual Tauri v2 Linux dependencies (`libwebkit2gtk-4.1-dev`,
`libgtk-3-dev`, `librsvg2-dev`, `patchelf`, `libxdo-dev`); see
[docs/office-console-stage-1.md](docs/office-console-stage-1.md) and
[docs/office-console-stage-2.md](docs/office-console-stage-2.md),
[docs/office-console-stage-3.md](docs/office-console-stage-3.md),
[docs/office-console-stage-4.md](docs/office-console-stage-4.md),
[docs/office-console-stage-5.md](docs/office-console-stage-5.md),
[docs/office-console-stage-6-ui.md](docs/office-console-stage-6-ui.md),
[docs/office-console-stage-7-design.md](docs/office-console-stage-7-design.md),
[docs/office-console-stage-8-flat-ui.md](docs/office-console-stage-8-flat-ui.md).
