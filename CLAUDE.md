# Working rules

- Never add a test unless explicitly asked to. Do fix any test that fails
  because of a change you made — run `cargo test` after every change and
  resolve failures before finishing.
- Never run the `--ignored` headless UI tests, render the app, or take a
  screenshot to check your own work. No UI verification of any kind — the
  user checks the UI themselves and will report back if something's off.
  The one exception: when a new feature made you modify an `--ignored` test,
  run all the `--ignored` tests afterwards and make them pass.
- Never run `git commit` (or push) unless explicitly asked.
- Comments: 3 lines max, always — this counts every line of the comment
  block, doc comments (`///`) included, and applies to existing comments you
  edit or extend too. State what something does or how it works — never why
  it changed, never a past bug or fix, never anything about a request that led
  to it. If an edit pushes a comment past 3 lines, shorten it instead. Count
  the lines of every comment you write or touch before finishing.
- Translations: after adding or changing an `@tr(...)` text in the `.slint` files,
  run `cargo xtask i18n`; never add or edit entries of `lang/cachemire.pot` or the
  `.po` files by hand. Then only fill in the `msgstr` of what the task reports as
  untranslated, and run it again until nothing is. Messages built in Rust
  (`t!(...)`) live in `locales/app.yml`, which has no task.

# Environment

- On Linux, ripgrep (`rg`) is available: use it instead of `grep` when searching.
- On Windows, the Rust uutils coreutils are installed, so the usual Unix commands
  (`ls`, `cat`, `cp`, …) work there too.
