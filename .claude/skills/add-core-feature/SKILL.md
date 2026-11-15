---
name: add-core-feature
description: Use when adding or extending a core backend in cachemire-core (connection, SSH tunnel, AWS, secret vault, session operation) and its error handling and UI glue.
---

# Add a core feature

1. Code in `crates/cachemire-core/src/<area>/` (`aws/mod.rs`, `connection/ssh_tunnel/mod.rs`, `secret_vault/mod.rs`, `session/`).
2. Errors: add a variant to the area's sub-enum in `src/error.rs` (English `Display`, thiserror).
3. App side: a `Localize` arm in `crates/cachemire/src/error/mod.rs` + entry in `resources/lang/app.yml` (English key, `fr:` value).
   `crates/cachemire/src/error/tests.rs` fails if `localized()` in English differs from `Display`.
4. UI glue: imitate `crates/cachemire/src/cloud/mod.rs` (spawn on `app.rt`, return via `app::later`).
5. Secrets are `Secret` (`src/secret.rs`), never `String`. No `unwrap` outside tests. Keep sibling `tests.rs` passing.
6. Run `cargo test`; `cargo xtask e2e` if it touches Redis behavior.

Avoid: importing Slint or rust-i18n into core; logging secrets; Windows-only code without `#[cfg(windows)]`.
