use super::error::AppError;
use super::session::Session;
use super::state::AppState;

/// Bytes of memory the server reports using — just the `memory` section of
/// `INFO`, not the whole (several KB) reply.
pub async fn used_memory(state: &AppState, connection_id: Option<&str>) -> Result<i64, AppError> {
    let raw = Session::open(state, connection_id).await?.info("memory").await?;
    Ok(raw
        .lines()
        .find_map(|line| line.strip_prefix("used_memory:"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0))
}

pub fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
