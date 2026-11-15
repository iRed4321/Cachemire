//! The wiki's chapters, only built by `cargo wiki`: each one is a Markdown file of `wiki/`.

#[doc = include_str!("../wiki/architecture.md")]
pub mod architecture {}

#[doc = include_str!("../wiki/runtime.md")]
pub mod runtime {}

#[doc = include_str!("../wiki/state.md")]
pub mod state {}

#[doc = include_str!("../wiki/errors.md")]
pub mod errors {}

#[doc = include_str!("../wiki/ui.md")]
pub mod ui {}

#[doc = include_str!("../wiki/keys_and_tabs.md")]
pub mod keys_and_tabs {}

#[doc = include_str!("../wiki/connections.md")]
pub mod connections {}

#[doc = include_str!("../wiki/redisql.md")]
pub mod redisql {}

#[doc = include_str!("../wiki/settings.md")]
pub mod settings {}

#[doc = include_str!("../wiki/i18n.md")]
pub mod i18n {}

#[doc = include_str!("../wiki/platform.md")]
pub mod platform {}

#[doc = include_str!("../wiki/testing.md")]
pub mod testing {}

#[doc = include_str!("../wiki/glossary.md")]
pub mod glossary {}
