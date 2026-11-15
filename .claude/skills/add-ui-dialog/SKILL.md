---
name: add-ui-dialog
description: Use when adding a dialog, panel or screen, or a new Slint callback handled in Rust (new .slint component plus its Rust init module).
---

# Add a dialog / callback

1. Callbacks and properties go in a global in `crates/cachemire/ui/globals.slint`; types in `ui/components/types.slint`.
2. Component in `ui/components/<name>.slint`, imported and bound in `ui/main.slint`. Model: `host-key-dialog.slint`.
3. Rust module with `pub fn init(window: &MainWindow)` attaching `window.global::<G>().on_<cb>(..)`.
   Models: `src/host_key_dialog.rs`, `src/keyring_dialogs.rs`. Register the `mod` and call `init` in `src/main.rs`.
4. Async work: `app::with(|app, _| app.rt.spawn(..))`, return via `app::later` (`src/app.rs`). Never block the UI thread.
5. Colors only from `theme.slint`. Texts with `@tr(...)`, then `cargo xtask i18n`. Errors shown via the `Localize` trait.

Avoid: `slint::private_unstable_api` outside `src/styled_text.rs`; business logic in the app crate (put it in core).
