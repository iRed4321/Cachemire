//! CI reports. `report-build` saves what the last `cargo … --timings` build compiled (with the packages
//! made from it, or the results of the tests run from it) as JSON in `target/reports/`; `report` merges
//! the saved ones, from every job of a run, into a single Markdown summary comparing the operating systems.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::tools::{Result, repo_root, summary};

/// The compile time from which a unit is listed among the slow ones.
const SLOW: f64 = 10.0;
/// How many slow units a step lists at most.
const MAX_SLOW: usize = 10;
/// The operating systems in the order of the report's columns, with their emoji.
const SYSTEMS: [(&str, &str); 3] = [("linux", "🐧 Linux"), ("windows", "🪟 Windows"), ("macos", "🍎 macOS")];

/// Saves the last timed build as `step` of this OS in `target/reports/`, with the `packages` made from it,
/// the results of the test runs of `tests` (name and nextest JUnit file) and the summary of `cargo llvm-cov
/// report --json --summary-only` in `coverage`; a missing test or coverage file is skipped.
pub fn save(step: &str, packages: &[&Path], tests: &[(&str, &Path)], coverage: Option<&Path>) -> Result<()> {
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
        "units": unit_data(&html)?,
        "packages": packages,
        "tests": test_results(tests),
        "coverage": coverage.filter(|file| file.is_file()).map(coverage_summary).transpose()?,
    });
    let dir = repo_root().join("target/reports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let slug: String = step.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let file = dir.join(format!("{slug}-{}.json", std::env::consts::OS));
    std::fs::write(&file, report.to_string()).map_err(|e| format!("{}: {e}", file.display()))?;
    println!("Report saved: {}", file.display());
    Ok(())
}

/// The value of attribute `key` in the tag text `tag` (`name="…"`), as written.
fn attribute<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    let start = tag.find(&format!(" {key}=\""))? + key.len() + 3;
    Some(&tag[start..start + tag[start..].find('"')?])
}

/// `text` with the XML entities a test name can hold turned back into characters.
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// A test run's results from nextest's JUnit file: the totals of its `<testsuites>` element and the names
/// of the `<testcase>`s holding a `<failure>` or an `<error>`; `None` when it has no `<testsuites>`.
fn parse_junit(xml: &str) -> Option<Value> {
    let root = xml.split("<testsuites ").nth(1).map(|rest| &rest[..rest.find('>').unwrap_or(rest.len())])?;
    let number = |key: &str| attribute(root, key).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let mut failures = Vec::new();
    for case in xml.split("<testcase ").skip(1) {
        let head = &case[..case.find('>').unwrap_or(case.len())];
        let body = &case[..case.find("</testcase>").unwrap_or(case.len())];
        if !head.ends_with('/') && (body.contains("<failure") || body.contains("<error")) {
            failures.push(unescape(attribute(&format!(" {head}"), "name").unwrap_or("?")));
        }
    }
    let failed = (number("failures") + number("errors")) as u64;
    Some(json!({ "passed": (number("tests") as u64).saturating_sub(failed), "failed": failed, "time": number("time"), "failures": failures }))
}

/// The test runs of `tests` (name and JUnit file) as JSON; a run with no file is left out.
fn test_results(tests: &[(&str, &Path)]) -> Value {
    let mut results = Vec::new();
    for (name, file) in tests {
        let Ok(xml) = std::fs::read_to_string(file) else { continue };
        let mut result = parse_junit(&xml).unwrap_or_else(|| json!({ "crashed": true }));
        result["name"] = json!(name);
        results.push(result);
    }
    Value::Array(results)
}

/// The totals of an llvm-cov JSON summary, and its lines by module of `crates/cachemire/src` (a folder,
/// or a file at its top), each as `[covered, count]`.
fn coverage_summary(file: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let summary: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let data = &summary["data"][0];
    let pair = |of: &Value, key: &str| [of[key]["covered"].as_u64().unwrap_or(0), of[key]["count"].as_u64().unwrap_or(0)];
    let mut modules = std::collections::BTreeMap::<String, [u64; 2]>::new();
    for entry in data["files"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let name = entry["filename"].as_str().unwrap_or_default().replace('\\', "/");
        let module = match name.split_once("crates/cachemire/src/") {
            Some((_, rest)) => rest.split_once('/').map_or(rest.to_string(), |(folder, _)| format!("{folder}/")),
            None => "other".to_string(),
        };
        let [covered, count] = pair(&entry["summary"], "lines");
        let total = modules.entry(module).or_default();
        *total = [total[0] + covered, total[1] + count];
    }
    let totals = &data["totals"];
    Ok(json!({ "lines": pair(totals, "lines"), "functions": pair(totals, "functions"), "regions": pair(totals, "regions"), "modules": modules }))
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
    units: Vec<(String, f64)>,
    packages: Vec<(String, u64)>,
    tests: Vec<Value>,
    coverage: Value,
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
                units: pairs("units").map(|(name, secs)| (name, secs.as_f64().unwrap_or(0.0))).collect(),
                packages: pairs("packages").map(|(name, size)| (name, size.as_u64().unwrap_or(0))).collect(),
                tests: value["tests"].as_array().cloned().unwrap_or_default(),
                coverage: value["coverage"].clone(),
            });
        }
    }
    Ok(())
}

fn render(title: &str, reports: &[Report]) -> String {
    let mut out = format!("## {title}\n\n");
    if reports.is_empty() {
        return out + "💤 Nothing was built: every job failed or stopped before building.\n";
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

    // the steps: build time per OS
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

    out += &test_section(reports, &systems);
    out += &coverage_section(reports, &systems);

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
    out += "<sub>⏱️ build time";
    if any_slow {
        out += &format!(" · `·` under {SLOW:.0} s");
    }
    if missing {
        out += " · — no report: the job failed, was skipped or had nothing to build";
    }
    out + "</sub>\n"
}

/// The test results of every step, a row per test run and a column per OS, then the failed tests by name.
fn test_section(reports: &[Report], systems: &[&(&str, &str)]) -> String {
    let mut names: Vec<&str> = Vec::new();
    for test in reports.iter().flat_map(|r| &r.tests) {
        let name = test["name"].as_str().unwrap_or("Tests");
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if names.is_empty() {
        return String::new();
    }
    let find = |name: &str, os: &str| reports.iter().rev().filter(|r| r.os == os).flat_map(|r| &r.tests).find(|t| t["name"] == name);
    let mut out = "#### 🧪 Test results\n\n| Tests |".to_string();
    for (_, label) in systems {
        out += &format!(" {label} |");
    }
    out += "\n|---|";
    out += &":-:|".repeat(systems.len());
    out += "\n";
    let mut failures = String::new();
    for name in &names {
        out += &format!("| **{name}** |");
        for (os, label) in systems {
            let count = |test: &Value, key: &str| test[key].as_u64().unwrap_or(0);
            out += &match find(name, os) {
                None => " — |".to_string(),
                Some(test) if test["crashed"] == true => " 💥 no result |".to_string(),
                Some(test) if count(test, "failed") > 0 => {
                    let names: Vec<String> = test["failures"].as_array().into_iter().flatten().filter_map(|n| Some(format!("`{}`", n.as_str()?))).collect();
                    failures += &format!("- **{name}** on {label}: {}\n", names.join(", "));
                    format!(" ❌ **{} failed** · {} passed |", count(test, "failed"), count(test, "passed"))
                }
                Some(test) => format!(" ✅ {} passed · {} |", count(test, "passed"), duration(test["time"].as_f64().unwrap_or(0.0))),
            };
        }
        out += "\n";
    }
    out += "\n";
    if !failures.is_empty() {
        out += &format!("**Failed tests**\n\n{failures}\n");
    }
    out
}

/// `[covered, count]` as a percentage, `None` when there's nothing to cover.
fn percent(pair: &Value) -> Option<f64> {
    let count = pair[1].as_f64().filter(|count| *count > 0.0)?;
    Some(100.0 * pair[0].as_f64().unwrap_or(0.0) / count)
}

/// The coverage measured on each OS: lines, functions and regions, then the lines by module, folded.
fn coverage_section(reports: &[Report], systems: &[&(&str, &str)]) -> String {
    let find = |os: &str| reports.iter().rev().find(|r| r.os == os && r.coverage.is_object()).map(|r| &r.coverage);
    if systems.iter().all(|(os, _)| find(os).is_none()) {
        return String::new();
    }
    let header = |first: &str, align: &str| {
        let mut out = format!("| {first} |");
        for (_, label) in systems {
            out += &format!(" {label} |");
        }
        out + "\n|---|" + &align.repeat(systems.len()) + "\n"
    };
    let dot = |p: f64| if p >= 80.0 { "🟢" } else if p >= 60.0 { "🟡" } else if p >= 40.0 { "🟠" } else { "🔴" };
    let mut out = "#### 🎯 Coverage of cachemire\n\n".to_string() + &header("", ":-:|");
    for (key, label) in [("lines", "Lines"), ("functions", "Functions"), ("regions", "Regions")] {
        out += &format!("| **{label}** |");
        for (os, _) in systems {
            out += &match find(os).and_then(|c| Some((percent(&c[key])?, &c[key]))) {
                Some((p, pair)) => format!(" {} **{p:.1}%** <sub>{}/{}</sub> |", dot(p), pair[0], pair[1]),
                None => " — |".to_string(),
            };
        }
        out += "\n";
    }
    let mut modules: Vec<&String> = systems.iter().filter_map(|(os, _)| find(os)?["modules"].as_object()).flat_map(|m| m.keys()).collect();
    modules.sort();
    modules.dedup();
    out += "\n<details><summary>Lines by module</summary>\n\n";
    out += &header("Module", "--:|");
    for module in modules {
        out += &format!("| `{module}` |");
        for (os, _) in systems {
            out += &match find(os).and_then(|c| percent(&c["modules"][module])) {
                Some(p) => format!(" {} {p:.1}% |", dot(p)),
                None => " — |".to_string(),
            };
        }
        out += "\n";
    }
    out + "\n</details>\n\n"
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
