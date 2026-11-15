---
name: translations
description: Use when adding or changing user-facing text, or a language (Slint @tr strings, Rust t! messages, French .po, app.yml).
---

# Translations

- Slint text: `@tr("English")`. Then `cargo xtask i18n` (crates/xtask/src/i18n.rs) updates `resources/lang/cachemire.pot`
  and `resources/lang/fr/LC_MESSAGES/cachemire.po`. Fill only the `msgstr` it reports as untranslated; rerun until clean.
  Never hand-edit `.pot`/`.po` otherwise.
- Rust text: `rust_i18n::t!("English text")`, keyed by the English text in `resources/lang/app.yml`
  (`fr:` value, `%{var}` placeholders). No xtask: edit by hand.
- New language: `LANGUAGES` and `Language` in `crates/cachemire-core/src/i18n/mod.rs`, `LanguageSetting` enum in Slint,
  `to_setting`/`from_setting` in `crates/cachemire/src/settings.rs`, new `resources/lang/<code>/`.

Avoid: forgetting the `app.yml` entry for new core error variants; changing an English key without updating `fr:`.
