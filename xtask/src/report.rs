//! CI reports. `report-build` saves what the last `cargo … --timings` build compiled (and the packages
//! made from it) as JSON in `target/reports/`; `report` merges the saved ones, from every job of a run,
//! into a single Markdown summary comparing the operating systems.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::tools::{Result, repo_root, summary};

/// The compile time from which a unit is listed among the slow ones.
const SLOW: f64 = 10.0;
/// How many slow units a step lists at most.
const MAX_SLOW: usize = 10;
/// The operating systems in the order of the report's columns, with their emoji.
const SYSTEMS: [(&str, &str); 3] = [("linux", "🐧 Linux"), ("windows", "🪟 Windows"), ("macos", "🍎 macOS")];

/// Saves the last timed build as `step` of this OS in `target/reports/`, with the `packages` made from it.
pub fn save(step: &str, packages: &[&Path]) -> Result<()> {
    let path = repo_root().join("target/cargo-timings/cargo-timing.html");
    let html = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e} (build with --timings first)", path.display()))?;
    let number = |key: &str| info(&html, key).and_then(|v| v.split(['s', ' ']).next()?.parse::<f64>().ok()).unwrap_or(0.0);
    let packages = (packages.iter())
        .map(|file| Ok(json!([file.file_name().unwrap_or_default().to_string_lossy(), std::fs::metadata(file).map_err(|e| format!("{}: {e}", file.display()))?.len()])))
        .collect::<Result<Vec<_>>>()?;
    let report = json!({
        "step": step,
        "os": std::env::consts::OS,
        "time": number("Total time"),
        "fresh": number("Fresh units"),
        "total": number("Total units"),
        "units": unit_data(&html)?,
        "packages": packages,
    });
    let dir = repo_root().join("target/reports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let slug: String = step.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let file = dir.join(format!("{slug}-{}.json", std::env::consts::OS));
    std::fs::write(&file, report.to_string()).map_err(|e| format!("{}: {e}", file.display()))?;
    println!("Report saved: {}", file.display());
    Ok(())
}

/// Merges the reports saved under `dir` (searched recursively) into one summary titled `title`:
/// printed, and added to the job summary in GitHub Actions.
pub fn run_task(title: &str, dir: Option<PathBuf>) -> Result<()> {
    let mut reports = Vec::new();
    collect(&dir.unwrap_or_else(|| repo_root().join("target/reports")), &mut reports)?;
    let markdown = render(title, &reports);
    println!("{markdown}");
    summary(&markdown)
}

/// One saved report: a step (tests, release build…) on one OS.
struct Report {
    step: String,
    os: String,
    time: f64,
    fresh: f64,
    total: f64,
    units: Vec<(String, f64)>,
    packages: Vec<(String, u64)>,
}

fn collect(dir: &Path, reports: &mut Vec<Report>) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    let mut paths: Vec<_> = entries.filter_map(|entry| entry.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(&path, reports)?;
        } else if path.extension().is_some_and(|ext| ext == "json") {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let value: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            let pairs = |key: &str| value[key].as_array().cloned().unwrap_or_default().into_iter().map(|pair| (pair[0].as_str().unwrap_or_default().to_string(), pair[1].clone()));
            reports.push(Report {
                step: value["step"].as_str().unwrap_or("Build").to_string(),
                os: value["os"].as_str().unwrap_or("?").to_string(),
                time: value["time"].as_f64().unwrap_or(0.0),
                fresh: value["fresh"].as_f64().unwrap_or(0.0),
                total: value["total"].as_f64().unwrap_or(0.0),
                units: pairs("units").map(|(name, secs)| (name, secs.as_f64().unwrap_or(0.0))).collect(),
                packages: pairs("packages").map(|(name, size)| (name, size.as_u64().unwrap_or(0))).collect(),
            });
        }
    }
    Ok(())
}

fn render(title: &str, reports: &[Report]) -> String {
    let mut out = format!("## {title}\n\n");
    if reports.is_empty() {
        return out + "💤 Nothing was built: every job found its work in the cache (or stopped before building).\n";
    }
    let systems: Vec<_> = SYSTEMS.iter().filter(|(os, _)| reports.iter().any(|r| r.os == *os)).collect();
    let find = |step: &str, os: &str| reports.iter().rev().find(|r| r.step == step && r.os == os);
    let mut steps: Vec<&str> = Vec::new();
    for report in reports {
        if !steps.contains(&report.step.as_str()) {
            steps.push(&report.step);
        }
    }
    // the longest step first: the one that decides how long the run takes
    let longest = |step: &&str| reports.iter().filter(|r| r.step == *step).map(|r| r.time).fold(0.0, f64::max);
    steps.sort_by(|a, b| longest(b).total_cmp(&longest(a)));
    let compare = systems.len() == 2 && systems[0].0 == "linux" && systems[1].0 == "windows";
    let mut missing = false;

    // the steps: build time and share of the units that came from the cache, per OS
    out += "| Build |";
    for (_, name) in &systems {
        out += &format!(" {name} |");
    }
    out += if compare { " 🪟 / 🐧 |\n|---|" } else { "\n|---|" };
    out += &":-:|".repeat(systems.len() + usize::from(compare));
    out += "\n";
    for step in &steps {
        out += &format!("| {} **{step}** |", emoji(step));
        for (os, _) in &systems {
            match find(step, os) {
                Some(r) if r.total > 0.0 => out += &format!(" ⏱️ **{}** · ♻️ {:.0} % |", duration(r.time), r.fresh / r.total * 100.0),
                Some(r) => out += &format!(" ⏱️ **{}** |", duration(r.time)),
                None => {
                    missing = true;
                    out += " — |";
                }
            }
        }
        if compare {
            match (find(step, "linux"), find(step, "windows")) {
                (Some(linux), Some(windows)) if linux.time > 0.0 => out += &format!(" ×{:.1} |", windows.time / linux.time),
                _ => out += " |",
            }
        }
        out += "\n";
    }
    out += "\n";

    // the slow units of each step, side by side
    let mut any_slow = false;
    for step in &steps {
        let mut slow: Vec<(&str, Vec<Option<f64>>)> = Vec::new();
        for (i, (os, _)) in systems.iter().enumerate() {
            for (name, secs) in find(step, os).map(|r| r.units.as_slice()).unwrap_or_default() {
                if *secs < SLOW {
                    continue;
                }
                let index = slow.iter().position(|(n, _)| n == name).unwrap_or_else(|| {
                    slow.push((name, vec![None; systems.len()]));
                    slow.len() - 1
                });
                slow[index].1[i] = Some(*secs);
            }
        }
        if slow.is_empty() {
            continue;
        }
        any_slow = true;
        let slowest = |times: &[Option<f64>]| times.iter().flatten().fold(0.0, |a: f64, &b| a.max(b));
        slow.sort_by(|a, b| slowest(&b.1).total_cmp(&slowest(&a.1)));
        out += &format!("#### 🐢 {step}: crates over {SLOW:.0} s\n\n| Crate |");
        for (_, name) in &systems {
            out += &format!(" {name} |");
        }
        out += "\n|---|";
        out += &"--:|".repeat(systems.len());
        out += "\n";
        for (name, times) in slow.iter().take(MAX_SLOW) {
            out += &format!("| `{name}` |");
            for (time, (os, _)) in times.iter().zip(&systems) {
                out += &match time {
                    Some(t) => format!(" {} |", duration(*t)),
                    None if find(step, os).is_some() => " · |".into(),
                    None => " — |".into(),
                };
            }
            out += "\n";
        }
        if slow.len() > MAX_SLOW {
            out += &format!("\n<sub>…and {} more over {SLOW:.0} s.</sub>\n", slow.len() - MAX_SLOW);
        }
        out += "\n";
    }
    if !any_slow {
        out += &format!("⚡ No crate took over {SLOW:.0} s to compile.\n\n");
    }

    // the packages
    let packages: Vec<_> = reports.iter().flat_map(|r| r.packages.iter().map(move |p| (&r.os, p))).collect();
    if !packages.is_empty() {
        out += "#### 📦 Packages\n\n| | File | Size |\n|:-:|---|--:|\n";
        for (os, (name, size)) in packages {
            let icon = SYSTEMS.iter().find(|(o, _)| o == os).map_or("", |(_, label)| label.split(' ').next().unwrap_or(""));
            out += &format!("| {icon} | `{name}` | {:.1} MB |\n", *size as f64 / 1_048_576.0);
        }
        out += "\n";
    }
    out += "<sub>⏱️ build time · ♻️ units taken from the cache";
    if any_slow {
        out += &format!(" · `·` under {SLOW:.0} s, or from the cache");
    }
    if missing {
        out += " · — no report: the job failed, was skipped or had nothing to build";
    }
    out + "</sub>\n"
}

/// The emoji of a step, from its name.
fn emoji(step: &str) -> &'static str {
    let step = step.to_lowercase();
    if step.contains("test") {
        "🧪"
    } else if step.contains("clippy") || step.contains("lint") {
        "📎"
    } else if step.contains("release") {
        "📦"
    } else {
        "🔨"
    }
}

/// The value of the timing report's `<td>{key}:</td><td>…</td>` row.
fn info(html: &str, key: &str) -> Option<String> {
    let needle = format!("<td>{key}:</td><td>");
    let start = html.find(&needle)? + needle.len();
    Some(html[start..start + html[start..].find("</td>")?].to_string())
}

/// The compiled units (`UNIT_DATA` of the timing report) as name and seconds, slowest first.
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
            // `name "bin"`, `name "bin" (test)`…: the kind, and the mode if any
            target => format!(" ({})", target.split_once(' ').map_or(target, |(_, kind)| kind).replace(['"', '(', ')'], "")),
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
