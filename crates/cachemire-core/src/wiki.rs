//! The wiki's chapters, only filled in by `cargo wiki`: each one is a Markdown file of `wiki/`.

#[cfg_attr(doc, doc = include_str!("../wiki/errors.md"))]
pub mod errors {}

#[cfg_attr(doc, doc = include_str!("../wiki/connection.md"))]
pub mod connection {}

#[cfg_attr(doc, doc = include_str!("../wiki/secrets.md"))]
pub mod secrets {}

#[cfg_attr(doc, doc = include_str!("../wiki/aws.md"))]
pub mod aws {}

#[cfg_attr(doc, doc = include_str!("../wiki/session.md"))]
pub mod session {}

#[cfg_attr(doc, doc = include_str!("../wiki/explorer.md"))]
pub mod explorer {}

#[cfg_attr(doc, doc = include_str!("../wiki/redisql.md"))]
pub mod redisql {}

#[cfg_attr(doc, doc = include_str!("../wiki/settings.md"))]
pub mod settings {}

#[cfg_attr(doc, doc = include_str!("../wiki/utilities.md"))]
pub mod utilities {}
