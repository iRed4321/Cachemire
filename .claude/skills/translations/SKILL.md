---
name: translations
description: Use when adding or changing user-facing text, or a language (Slint @tr strings, Rust t! messages, French .po, app.yml).
---

# Translations

- Slint text: `@tr("English")`. Then `cargo xtask i18n` (crates/xtask/src/i18n.rs) updates `resources/lang/cachemire.pot`
  and `resources/lang/fr/LC_MESSAGES/cachemire.po`. Fill only the `msgstr` it reports as untranslated; rerun until clean.
  Never hand-edit `.pot`/`.po` otherwise.
- Rust text: `t!("English text")` (the app's macro, `use crate::t;`), keyed by the English text in `resources/lang/app.yml`
  (`fr:` value, `%{var}` placeholders). No xtask: edit by hand. `build.rs` compiles it into a table (`src/i18n/`).
- A `t!("...")` written anywhere in `src/`, comments included, must have an `app.yml` entry (`i18n` test scans the text).
- New language: `LANGUAGES` and `Language` in `crates/cachemire-core/src/i18n/mod.rs`, `LanguageSetting` enum in Slint,
  `to_setting`/`from_setting` in `crates/cachemire/src/settings.rs`, new `resources/lang/<code>/`.

Avoid: forgetting the `app.yml` entry for new core error variants; changing an English key without updating `fr:`.
- A `//` comment on the line just above an `@tr(...)` line is taken as its translator comment: one long enough to wrap
  breaks the `.pot` (`cargo xtask i18n` then fails on it; `git checkout` the `.pot`, move the comment, rerun).
