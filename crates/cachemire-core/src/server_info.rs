use crate::error::Error;
use crate::session::Session;
use crate::state::AppState;

/// Bytes of memory the server reports using — just the `memory` section of
/// `INFO`, not the whole (several KB) reply.
pub async fn used_memory(state: &AppState, connection_id: Option<&str>) -> Result<i64, Error> {
    let raw = Session::open(state, connection_id).await?.info("memory").await?;
    Ok(raw.lines().find_map(|line| line.strip_prefix("used_memory:")).and_then(|v| v.trim().parse().ok()).unwrap_or(0))
}

pub fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{value:.0} {}", UNITS[unit]) } else { format!("{value:.1} {}", UNITS[unit]) }
}

#[cfg(test)]
mod tests {
    use super::format_bytes;

    #[test]
    fn sizes_are_shown_in_the_largest_unit_under_1024() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(4 * 1024 * 1024), "4.0 MB");
        assert_eq!(format_bytes(3 * 1024_i64.pow(3) / 2), "1.5 GB");
        assert_eq!(format_bytes(5 * 1024_i64.pow(5)), "5120.0 TB", "terabytes are the largest unit");
    }
}
