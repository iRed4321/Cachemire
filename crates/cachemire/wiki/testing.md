# Testing

How the tests are organised, what each layer covers, and what to run when. The project rule comes
first: **tests are only added when explicitly requested**; when a change breaks an existing test, that
test is fixed. After any change `cargo test` is run, and `cargo fmt --all` for Rust code.

## Where to look

- `src/**/tests.rs`: unit tests, one sibling file per module (`#[cfg(test)] mod tests;`).
- `src/ui_tests/`: headless UI tests with the `Harness` (`mod.rs`), by area (`app.rs`, `cloud.rs`,
  `connections.rs`, `dialogs.rs`, `export.rs`, `formats.rs`, `links.rs`, `shortcuts.rs`, `smart_box.rs`, `tabs.rs`, `themes.rs`) and `e2e/` against a real Redis.
- `crates/cachemire-core/src/e2e/`: core end-to-end tests; `src/test_support.rs`: shared helpers.
- `crates/xtask/src/e2e.rs`: `cargo xtask e2e`. `.config/nextest.toml`: the `ci` profile.
- `.github/workflows/ci.yml`: what CI runs.

## Four kinds of test

| Kind | Lives in | Needs | Runs with |
|---|---|---|---|
| Unit | `tests.rs` beside each module, both crates | nothing | `cargo test` |
| Headless UI | `crates/cachemire/src/ui_tests/*.rs` | nothing (software renderer) | `cargo test` |
| Core e2e | `crates/cachemire-core/src/e2e/` | a Redis server | `cargo xtask e2e` |
| App e2e | `crates/cachemire/src/ui_tests/e2e/` | a Redis server | `cargo xtask e2e` |

### Unit tests: sibling `tests.rs`

A module `foo` (a file or a folder with `mod.rs`) ends with `#[cfg(test)] mod tests;` and its tests are in
`foo/tests.rs` (`use super::*;` reaches private items). Examples worth reading:

- `settings/tests.rs`: the store with a `MemoryVault` and a temp file: profiles, vault behaviour, "a file that
  cannot be read is kept", the default profile seeded once.
- `persist/tests.rs`: atomic save, set-aside of a bad file, a failing save leaves the file as it was.
- `i18n/tests.rs` (app): completeness of the `.po` and `app.yml` against the template and the sources.
- `error/tests.rs` (app): every core error variant has a `Localize` arm and a translation.
- `state.rs` has its tests inline (`mod tests`), including the legacy-data migration.

Rule from the crate notes: keep the sibling `tests.rs` in step with the module (a refactor that moves
code moves its tests); `unwrap` is allowed in tests only.

### Core helpers: `test_support` and the `test-support` feature

`cachemire_core::test_support` is compiled `#[cfg(any(test, feature = "test-support"))]`. The app depends on
core with that feature only as a *dev*-dependency, so release builds do not contain it. It provides:

- `temp_dir(name)`: a new unique empty directory under the system temp dir.
- `block_on(future)`: runs a future on a throwaway current-thread runtime.
- `TestRedis::claim()`: a database of the e2e server for one test alone (see below), emptied, with a
  connection for setting up and checking; `state()` builds an `AppState` whose active connection `e2e`
  points to it; `url_as` logs in as an ACL user the test created.
- `e2e_state(url)`: the same state for any URL.

Related test-only constructors elsewhere: `SettingsStore::in_memory`, `ConnectionStore::in_memory` and
`AppState::for_tests(dir)` (all files in `dir`, secrets in memory, SSH host keys auto-accepted).
Secrets in tests use `MemoryVault` instead of the OS keyring (the core `secrets` wiki chapter).

### Headless UI tests: the `Harness`

`ui_tests::Harness::new()` installs a test Slint `Platform` backed by `MinimalSoftwareWindow` (1300x800,
`RepaintBufferType::NewBuffer`), creates the real `MainWindow`, shows it, selects the English translation and puts it in a
state where the key panel accepts input (connected, one tab, badge `HASH`, panel loaded, name `users`).
Tests then drive the real window:

- `click(x, y)` dispatches move, press and release; `mod.rs` also has `install_app`, `wait_for`, `sweep` and `wait_out_double_click`.
  `sweep` clicks a grid until a click does what is looked for: keep its area tight around the target,
  since every point costs a click and the `between`/`found` calls (a sleep in them lasts a ~16 ms timer
  tick on Windows), and a sweep over the whole window takes a minute there.
- `frame()` runs what other threads handed to the UI thread, updates timers and animations, and
  renders into `buffer` (RGB565); `settle()` loops it 30 times at 20 ms for animations and inertia.
- Assertions read Slint globals (`window.global::<KeyPanel>()...`) or the rendered pixels.
- Nothing reaches the desktop: under `cfg(test)` a copy or a value opened in an editor lands in
  `value_actions::HANDED` (read with `handed_out`), and the last toast in `toast::take_last()`.

Design constraints visible in `ui_tests/mod.rs`:

- Slint's context is thread-local and each test builds its own `Harness`; but Slint accepts only one
  event-loop proxy per process, so `TestPlatform` gives the proxy to the first platform only and
  `TestProxy::invoke_from_event_loop` pushes into a shared `HANDED` queue.
- `EventLoopTurn` is a global mutex: the one test that drives `app::later` (which reaches the app of the
  thread that runs it) holds the turn, so jobs handed by Tokio threads run on the right test's thread.
  The queue is cleared when a turn starts.
- Tests that need the whole wiring create an `App` (`crate::app::install`) and the relevant `init`s, as `main`
  does, with `AppState::for_tests`.

### End-to-end tests against a real Redis

Both e2e suites are `#[ignore = "needs a Redis server: cargo xtask e2e"]` (the core's cluster tests:
`"needs a Redis cluster: cargo xtask e2e"`), so a plain `cargo test` skips them.

`cargo xtask e2e` (`crates/xtask/src/e2e.rs`):

1. If `CACHEMIRE_E2E_REDIS` is set it uses that server (whose databases the tests empty).
2. Otherwise it picks a free port and starts, in this order, a local `redis-server` (Valkey, Memurai) on the PATH, a Docker
   container (`redis:7.4-alpine`), or a `redis-server` in WSL. The server runs with 10 000 databases and
   no persistence.
3. It waits until the server answers, then runs `cargo nextest run --run-ignored only -E 'test(/(^|::)e2e::/)'`
   (every test inside an `e2e` module of either crate) with the URL in the environment; without nextest it
   falls back to `cargo test -- --ignored e2e::`. Extra arguments go after `--` (`-- --profile ci`).
4. It also starts a cluster of three primaries (`CLUSTER_NODES`), the same first way that works (in Docker,
   all three in one container that publishes their ports), each with its own bus port and announcing
   `127.0.0.1`; it gives each a third of the slots (`CLUSTER ADDSLOTSRANGE`, Redis 7+), has the first
   `CLUSTER MEET` the others and waits for `cluster_state:ok` on all. `CACHEMIRE_E2E_CLUSTER` set to one node's
   URL uses that cluster instead (the tests write keys under `e2e:` into it and leave some there). With WSL
   as the only way (a cluster can stall WSL 1), no cluster is started and the cluster's tests are left out
   of the filter (`FILTER_NO_CLUSTER`). The CI runs them on both OSes: on Windows it puts a native Redis
   build (redis-windows, pinned and checked by its SHA-256) on the PATH instead of WSL, which can lock a runner up.
5. It shuts the servers down afterwards.

Isolation between tests: `TestRedis::claim` increments a counter key in database 0, takes database
`1 + (n - 1) % (databases - 1)` and `FLUSHDB`s it, so parallel tests and parallel test processes use
different databases. That is why the server is started with many databases. A cluster has one database:
`TestCluster::claim` gives each test a key prefix of its own (`e2e:<test>:<pid>:<ms>:`, no hash tag, so its
keys spread over the slots), and the cluster tests only look at their own keys.

Core e2e areas: `connection`, `browse`, `read`, `edit`, `export`, `search`, `redisql`, and `cluster` (connecting
through one node, walking every primary, a level, a search, renames across slots, a folder moved and deleted). The app's `ui_tests/e2e`
(`tree.rs`, `values.rs`, `big_keys.rs`, `tabs.rs`, `connections.rs`, `secrets.rs`) start a `Running` struct: seed the database, build a `Harness`, connect through
`connections::connect_to`, wait for the tree, then act like a user (these also hold an `EventLoopTurn`).

## CI

`.github/workflows/ci.yml`, per OS (Ubuntu and Windows), skipped when only docs, workflows or text changed
on a pull request:

```text
cargo fmt --all --check            (Linux)
cargo deny check                   (Linux): advisories, licences, sources
cargo clippy --all-targets -- -D warnings
cargo nextest run --no-run         build the tests, under cargo-llvm-cov's environment
cargo nextest run --profile ci     unit + headless UI tests; results to target/nextest/ci/junit.xml
xtask e2e -- --profile ci          end-to-end tests (Windows: a native Redis build on the PATH)
cargo llvm-cov report              coverage, test-only files excluded
```

The `ci` nextest profile has `fail-fast = false` and writes JUnit XML for the summary job
(`xtask report-build`); coverage excludes `test_support`, `ui_tests`, `e2e` and `tests` files. Release
workflows run `cargo test` before building. Doctests: not established whether CI runs them (nextest does not;
the core crate's rule that wiki code blocks are ```` ```text ```` exists so a doctest run does not try to compile them).

## What to run when

| Change | Run |
|---|---|
| any Rust change | `cargo test`, then `cargo fmt --all`; last step before handing back: `cargo build --profile fast` |
| `.slint` text (`@tr`) | `cargo xtask i18n` until nothing is untranslated; `i18n/tests.rs` is the safety net |
| a new `t!` message or error variant | add the `app.yml` entry (tests fail otherwise) |
| session, search, redisql execution, connection code | also `cargo xtask e2e` (needs Redis, Docker or WSL) |
| UI layout or callbacks | `cargo test` runs the headless tests; the project does not check the UI by launching the app |
| docs or wiki | `cargo wiki` (alias for `cargo doc --no-deps --document-private-items`): broken intra-doc links fail |

## Gotchas

- A test that touches the data directory must use `temp_dir` / `for_tests`; `resolve_data_file` is cached
  per process and points to the user's real directory.
- UI tests share a process: do not rely on global Slint state from another test, and never call
  `app::later` from a test that has not taken an `EventLoopTurn`.
- e2e tests silently do not run in `cargo test`; a green `cargo test` says nothing about them.
