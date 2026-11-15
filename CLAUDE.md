# Cachemire

Desktop Redis/Valkey client (GPL-3.0-only): Slint UI (forked), Tokio, SSH tunnels, AWS ElastiCache/SSO,
OS keyring, Redisql query language. UI text is English + French.

## Workflow

- At session start, read `NOTES.md` if it exists (working memory between sessions).
- Delegate broad exploration (sweeping many files) to subagents; keep the main context for edits.
- When you discover a convention or pitfall, update the matching skill or sub-folder CLAUDE.md
  (or this file if it is cross-cutting). Log in-progress work and decisions in `NOTES.md`.

## Architecture

- `crates/cachemire-core/` UI-free library (connections, session, redisql, secret vault, settings, aws). See its CLAUDE.md.
- `crates/cachemire/` Slint app: `src/` Rust glue per feature, `ui/` .slint files. See its CLAUDE.md.
- `crates/xtask/` dev and release tooling (`cargo xtask`, `cargo ship`).
- `crates/installer-test/` stand-in app to try the MSI installer quickly.
- `resources/` icons, desktop entry, MSI, fonts, `lang/` (`cachemire.pot`, `fr/…/cachemire.po`, `app.yml`).
- `docs/` `releasing.md` (branch model, `cargo ship`), `slint-fork.md` (the Slint fork, upgrades).
- Default members: `cachemire` + `cachemire-core`. Edition 2024, resolver 3, no pinned toolchain.

## Commands

- Build/run: `cargo build --profile fast` / `cargo run --profile fast` (release without LTO, in `target/fast/`).
- Test: `cargo test` (headless Slint UI tests in `crates/cachemire/src/ui_tests/` included).
- E2E: `cargo xtask e2e` (needs redis-server, valkey-server, Docker or WSL; forwards args to nextest).
- Lint (CI): `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo deny check`.
- i18n: `cargo xtask i18n`. Packaging: `cargo xtask build-linux|build-deb|build-appimage|build-windows`.
- Releases: `cargo ship …` only when asked (see `docs/releasing.md`).

## Conventions

- Lints (`[workspace.lints]`): `unwrap_used`, `dbg_macro`, `todo` warn (CI fails on warnings); `unsafe_op_in_unsafe_fn` denied.
  Every `unsafe` has a `// SAFETY:` line. rustfmt: max_width 140.
- Secrets (password, passphrase, token) use `cachemire_core::secret::Secret`, never `String`; `.expose()` to read.
- Errors: typed `Error` in core (English `Display`), translated in the app by the `Localize` trait.
- Modules use the `xxx/mod.rs` layout (keep it), tests are sibling `tests.rs`/`tests/`. `parking_lot` mutexes.
- Logging with `tracing`; Windows-only code behind `#[cfg(windows)]`.
- Left alone on purpose: thread_local `App`, version `0.0.0-dev` in Cargo.toml (injected at release).

## Pitfalls

- Slint is a git fork: `slint` and `slint-build` revs in `crates/cachemire/Cargo.toml` move together;
  Renovate ignores them. `slint::private_unstable_api` may only appear in `crates/cachemire/src/styled_text.rs`.
- russh and rustls use `ring`, not aws-lc-rs (no cmake/nasm on Windows).
- `deny.toml` lists accepted advisories with reasons; any other finding is new. Add new ones with a reason.
- `cargo xtask`/`cargo ship` build into `target/xtask`, not the app's target dir.
- CI: workflows run `xtask …` (a cached or downloaded binary), not `cargo xtask`. A branch only reads its own caches
  and `main`'s (`release/*` never sees `next`'s): share between jobs with artifacts. Betas build with `--fast --as-version`.
  Details in `docs/releasing.md` ("xtask in the workflows").
- Worktree agents start from `main`, not `next`: `git reset --hard` to the right commit first.
  Never build under `/tmp`; max 3 builds at once (`CARGO_BUILD_JOBS=3`); delete each worktree `target/`.
- Secret vault and AWS SSO are not testable against a real keyring/AWS here: the user checks by hand.

## Skills (`.claude/skills/`)

`add-setting`, `add-ui-dialog`, `add-core-feature`, `translations`, `redisql-feature`.

## Working rules

- Never add a test unless explicitly asked to. Do fix any test that fails
  because of a change you made — run `cargo test` after every change and
  resolve failures before finishing.
- Run `cargo fmt --all` after changing Rust code; CI fails on unformatted code
  (`cargo fmt --all --check`).
- Never launch the app or take a screenshot to check your own work. No UI
  verification of any kind beyond the tests — the user checks the UI
  themselves and will report back if something's off.
- After finishing all the requested changes, run `cargo build --profile fast`
  as the last step before handing back, so the user can test that build right
  away. Fix any build failure first.
- Never run `git commit` (or push) unless explicitly asked.
- Branches: work goes on `next` (what the next minor brings), and urgent fixes of the
  stable release on a `fix/<name>` branch made by `cargo ship start-fix`. `main` only
  changes through the pull requests `cargo ship` opens: never commit to it. Never switch
  branches or run `cargo ship` unless asked. The process: `docs/releasing.md`.
- Comments: 3 lines max, always — this counts every line of the comment
  block, doc comments (`///`) included, and applies to existing comments you
  edit or extend too. State what something does or how it works — never why
  it changed, never a past bug or fix, never anything about a request that led
  to it. If an edit pushes a comment past 3 lines, shorten it instead. Count
  the lines of every comment you write or touch before finishing.
- Translations: after adding or changing an `@tr(...)` text in the `.slint` files,
  run `cargo xtask i18n`; never add or edit entries of `resources/lang/cachemire.pot` or the
  `.po` files by hand. Then only fill in the `msgstr` of what the task reports as
  untranslated, and run it again until nothing is. Messages built in Rust
  (`t!(...)`) live in `resources/lang/app.yml`, which has no task.
- Changelog: after a substantial change a user would notice (a feature, a fix, a
  changed behavior or limit), add or update its line under `## Unreleased` in
  `CHANGELOG.md`, in the subsection it belongs to; create `## Unreleased` at the
  top if it's missing. Write for users: what changed for them, not how it's built.
  Under Fixes, keep each line to 20 words at most and start it with its
  theme in bold, then a colon (`**Window**: `, `**Export**: `, `**Security**: `…),
  reusing an existing theme when one fits; keep lines of the same theme together,
  themes in alphabetical order.
  Internal work (refactors, CI, tooling) gets no line, except a dependency update
  that fixes a security advisory: give it a **Security** line under Fixes, naming
  what it protects and the advisory IDs. Never rename `## Unreleased`
  or write a version heading: `cargo ship stable` and `cargo ship fix` do that.

## Environment

- On Linux, ripgrep (`rg`) is available: use it instead of `grep` when searching.
- On Windows, the Rust uutils coreutils are installed, so the usual Unix commands
  (`ls`, `cat`, `cp`, …) work there too.
