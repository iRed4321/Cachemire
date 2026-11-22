---
name: add-setting
description: Use when adding or changing a user preference/toggle (persisted setting shown in the Settings dialog), end to end from core field to Slint control.
---

# Add a setting

Files, in order:
1. `crates/cachemire-core/src/settings/mod.rs`: add the field (with a serde default so old files still load) and its persistence.
2. `crates/cachemire/ui/globals.slint`: property + callback on the `Preferences` global.
3. `crates/cachemire/ui/components/settings.slint`: the control, bound to `Preferences`.
4. `crates/cachemire/src/settings.rs`: initial `set_<prop>` from the loaded settings, `on_<callback>` saving it.
   Imitate an existing toggle such as `restore-session-toggled` (`rg -n restore_session_toggled crates`).
5. Enum settings: mirror the Slint enum in `ui/components/types.slint` with exhaustive `to_setting`/`from_setting`.
6. `@tr(...)` texts: run `cargo xtask i18n`, fill the French `msgstr` (skill `translations`).
7. User-visible: line under `## Unreleased` in `CHANGELOG.md`.

Avoid: breaking loading of existing settings files; hand-editing `.po`/`.pot`; adding tests unless asked.
