# cachemire (Slint app)

UI-thread glue between `ui/*.slint` and `cachemire-core`. No business logic here.

## Layout

- `src/main.rs`: declares modules, `slint::include_modules!()` (re-exported as `ui::*`), backend choice.
- `build.rs`: compiles `ui/main.slint` with bundled translations.
- `src/theme/`: the themes (four colors each, the rest derived) and syntax themes; fills the Slint `Theme` global and
  `theme::paint()`, the colors of the text Rust paints.
- `src/app.rs`: `App` (thread_local). `app::with(|app, _| ..)` reaches it; `app::later` returns Tokio results to the UI thread.
- One module per feature (`connections/`, `cloud/`, `key_detail/`, `keyspace/`, `search/`, `redisql/`, `tabs/`…),
  each with an `init` that attaches the `on_*` callbacks; `init`s are called from `main.rs`.
- `ui/main.slint` root window; `ui/globals.slint` globals (callbacks Rust answers); `ui/components/*.slint` widgets.
- `src/i18n/`: the `t!` macro, over the table `build.rs` makes of `resources/lang/app.yml`.
- `src/error/`: `Localize` trait turning core errors into translated text.
- `src/styled_text.rs`: the only place allowed to name `slint::private_unstable_api`.

## Wiki chapters (`wiki/`)

`architecture`, `runtime` (threads, `App`), `state`, `errors`, `ui`, `keys_and_tabs`, `connections`, `redisql`, `settings`,
`i18n`, `platform` (windows, links, updates), `testing`, `glossary`. Start at `wiki/index.md`.

## Rules

- Flow: Slint callback -> handler -> `app::with` -> spawn on `app.rt` -> `app::later` -> set properties / `VecModel`.
- Slint enums mirrored in core use exhaustive matches (`to_setting`/`from_setting` in `src/settings.rs`).
- Colors: Slint reads the `Theme` global, Rust `theme::paint()`; never hard-code `#rrggbb` elsewhere. A new interface
  color is a `Theme` `in property` (default = the default theme's value) derived in `theme::derive` and set in `theme::apply`.
- The UI is compiled with Slint's language edition 1.19 (`build.rs`): an `if`/`for` child counts toward its parent's
  size, and a binding loop is an error. An overlay (dialog) sets `x` and `y` so it stays out of its parent's size.
- UI tests: `src/ui_tests/` (`Harness` in `mod.rs`, headless, software renderer). Add only when asked.
- After editing `@tr(...)`: `cargo xtask i18n`. Skills: `add-setting`, `add-ui-dialog`, `translations`.
