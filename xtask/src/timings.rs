//! `cargo xtask timings`: a Markdown report of the last `cargo … --timings` build (what it built and
//! its slowest units), read from `target/cargo-timings/cargo-timing.html`. Printed, and added to the
//! job summary when run in GitHub Actions.

use serde_json::Value;

use crate::tools::{Result, repo_root, summary};

/// How many of the slowest units the report lists.
const SLOWEST: usize = 15;

/// Prints the report of the last timed build under `title`, and adds it to the job summary.
pub fn publish(title: &str) -> Result<()> {
    let markdown = report(title)?;
    println!("\n{markdown}");
    summary(&markdown)
}

fn report(title: &str) -> Result<String> {
    let path = repo_root().join("target/cargo-timings/cargo-timing.html");
    let html = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e} (build with --timings first)", path.display()))?;
    let field = |key: &str| info(&html, key).unwrap_or_else(|| "?".into());
    let units = unit_data(&html)?;

    let mut markdown = format!("### {title}\n\n| | |\n|---|---|\n");
    markdown += &format!("| Profile | {} |\n", field("Profile"));
    markdown += &format!("| Units | {} in all: {} from the cache, {} compiled |\n", field("Total units"), field("Fresh units"), field("Dirty units"));
    markdown += &format!("| Total time | {} |\n", field("Total time"));
    let busy: f64 = units.iter().map(|unit| unit.1).sum();
    markdown += &format!("| Compile time, all units added up | {} |\n", duration(busy));
    markdown += &format!("| Max concurrency | {} |\n", field("Max concurrency"));
    markdown += &format!("| Compiler | {} |\n\n", field("rustc"));
    if units.is_empty() {
        markdown += "Nothing was compiled: every unit came from the cache.\n";
        return Ok(markdown);
    }
    markdown += &format!("#### Slowest units ({} of {})\n\n| # | Unit | Time | Share |\n|---:|---|---:|---:|\n", units.len().min(SLOWEST), units.len());
    for (i, (name, seconds)) in units.iter().take(SLOWEST).enumerate() {
        markdown += &format!("| {} | `{name}` | {} | {:.0} % |\n", i + 1, duration(*seconds), seconds / busy * 100.0);
    }
    Ok(markdown)
}

/// The value of the report's `<td>{key}:</td><td>…</td>` row, its line breaks made ` · `.
fn info(html: &str, key: &str) -> Option<String> {
    let needle = format!("<td>{key}:</td><td>");
    let start = html.find(&needle)? + needle.len();
    let value = &html[start..start + html[start..].find("</td>")?];
    Some(value.replace("<br>", " · "))
}

/// The compiled units (`UNIT_DATA` of the report) as name and seconds, slowest first.
fn unit_data(html: &str) -> Result<Vec<(String, f64)>> {
    let start = html.find("const UNIT_DATA = ").ok_or("no UNIT_DATA in the timing report")? + 18;
    let end = start + html[start..].find("\n];").ok_or("UNIT_DATA is not closed")? + 2;
    let data: Value = serde_json::from_str(&html[start..end]).map_err(|e| format!("UNIT_DATA: {e}"))?;
    let text = |unit: &Value, key: &str| unit[key].as_str().unwrap_or_default().to_string();
    let label = |unit: &Value| {
        let kind = match text(unit, "target").trim() {
            "" => String::new(),
            "build-script" => " (build script)".into(),
            "build-script (run)" => " (build script run)".into(),
            target => format!(" ({})", target.rsplit(' ').next().unwrap_or_default().trim_matches('"')),
        };
        format!("{} {}{kind}", text(unit, "name"), text(unit, "version"))
    };
    let mut units: Vec<_> = (data.as_array().map(Vec::as_slice).unwrap_or_default().iter())
        .map(|unit| (label(unit), unit["duration"].as_f64().unwrap_or(0.0)))
        .collect();
    units.sort_by(|a, b| b.1.total_cmp(&a.1));
    Ok(units)
}

/// `seconds` as `42.0s` or `3m 05s`.
fn duration(seconds: f64) -> String {
    let whole = seconds.round() as u64;
    if seconds < 60.0 { format!("{seconds:.1}s") } else { format!("{}m {:02}s", whole / 60, whole % 60) }
}
