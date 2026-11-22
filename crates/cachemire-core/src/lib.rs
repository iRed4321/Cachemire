#![doc = include_str!("../wiki/index.md")]
#![cfg_attr(doc, allow(rustdoc::private_intra_doc_links))]

use std::time::{SystemTime, UNIX_EPOCH};

pub mod aws;
pub mod connection;
pub mod error;
pub mod explorer;
pub mod export_files;
pub mod i18n;
pub mod json_path;
pub mod key_name;
pub mod link;
pub mod persist;
pub mod redis_value_type;
pub mod redisql;
pub mod runtime;
pub mod secret;
pub mod secret_vault;
pub mod server_info;
pub mod session;
pub mod settings;
pub mod state;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod updates;
pub mod wiki;

// against a real Redis server: ignored unless run by `cargo xtask e2e`
#[cfg(test)]
mod e2e;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
