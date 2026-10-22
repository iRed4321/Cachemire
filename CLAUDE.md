# Working rules

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

# Environment

- On Linux, ripgrep (`rg`) is available: use it instead of `grep` when searching.
- On Windows, the Rust uutils coreutils are installed, so the usual Unix commands
  (`ls`, `cat`, `cp`, …) work there too.
