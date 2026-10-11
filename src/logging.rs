//! The app's log: a file per day in `logs/` beside the saved data (the last week is
//! kept), and stderr in debug builds. `CACHEMIRE_LOG` takes an `EnvFilter` directive.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

const DEFAULT_FILTER: &str = "warn,cachemire=info";

/// Starts logging into `dir`; the guard flushes the file when dropped, so it is kept
/// until the app ends. Panics are logged too, then reported as before.
pub fn init(dir: &Path) -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_env("CACHEMIRE_LOG").unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("cachemire")
        .filename_suffix("log")
        .max_log_files(7)
        .build(dir);
    let (file, guard) = match appender {
        Ok(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            (Some(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(writer)), Some(guard))
        }
        Err(e) => {
            eprintln!("can't write the log to {}: {e}", dir.display());
            (None, None)
        }
    };
    let stderr = cfg!(debug_assertions).then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr));
    tracing_subscriber::registry().with(filter).with(file).with(stderr).init();

    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("{info}");
        report(info);
    }));
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "Cachemire starting");
    guard
}
