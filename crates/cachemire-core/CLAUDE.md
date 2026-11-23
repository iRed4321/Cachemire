# cachemire-core (UI-free library)

Must not depend on Slint or the app's `t!` messages. Feature `test-support` exposes helpers to the app's tests.

## Layout (`src/`)

- `connection/`: `client.rs`, `pool.rs`, `tls.rs`, `url.rs`, `store/` (saved connections), `ssh_tunnel/` (russh).
- `aws/`: profiles, ElastiCache, credentials; `sso.rs` browser SSO (PKCE, shared `~/.aws/sso/cache`).
- `secret.rs` (`Secret`, zeroized, `Debug` prints `***`), `secret_vault/` (OS keyring via keyring-core).
- `settings/` + `persist/`: preferences and profiles stored as JSON.
- `session/`: per-connection operations, `search.lua`; `explorer.rs` key tree; `key_name/`.
- `redisql/`: lexer, ast, parser, exec, complete, history, script.
- `state.rs` (`AppState`, `InstanceLock`), `runtime.rs` (Tokio), `error.rs`, `i18n/` (`Language`, `LANGUAGES`).
- `e2e/`: tests against a real Redis, run with `cargo xtask e2e`. `test_support.rs`: `temp_dir`, `block_on`, `TestRedis`.

## Wiki chapters (`wiki/`)

`errors`, `connection`, `secrets`, `aws`, `session`, `explorer`, `redisql`, `settings`, `utilities` (json_path, key_name, export_files).
Start at `wiki/index.md`.

## Rules

- Error text is English in `Display`; a new variant needs a `Localize` arm in `crates/cachemire/src/error/mod.rs`
  AND an `app.yml` entry (`i18n/tests.rs` checks every `t!("…")` has one; `error/tests.rs`, English = `Display`).
- Any password/passphrase/token is a `Secret`. Plain-text save asks the user first; SSH host keys are confirmed.
- `unwrap` only in tests. Keep sibling `tests.rs` in step with the module.
- Skills: `add-core-feature`, `redisql-feature`.
- Clusters: a command, pipeline or script over keys of different slots fails (`CROSSSLOT`). Send commands about many
  keys through `Session::replies` (one pipeline on a server, one command each on a cluster); walk with `KeyScan`.
