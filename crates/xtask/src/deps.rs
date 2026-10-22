//! The dependency updates between two commits, as changelog lines: from the difference of their
//! `Cargo.lock`, and the advisories `cargo audit` reports on one and not the other.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::tools::{Result, cargo, git};

/// The versions of each package of a `Cargo.lock`, the workspace's own (with no `source`) left out.
fn packages(lock: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut packages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for block in lock.split("[[package]]").skip(1) {
        let field = |key: &str| {
            block.lines().find_map(|line| line.strip_prefix(&format!("{key} = \""))).map(|value| value.trim_end().trim_end_matches('"'))
        };
        if let (Some(name), Some(version), Some(_)) = (field("name"), field("version"), field("source")) {
            packages.entry(name.to_string()).or_default().insert(version.to_string());
        }
    }
    packages
}

/// The advisories `cargo audit` reports for the lock file at `path`, by id, each with its package.
fn advisories(root: &Path, path: &Path, fetch: bool) -> Result<BTreeMap<String, String>> {
    let output = cargo()
        .current_dir(root)
        .args(["audit", "--json", "--file"])
        .arg(path)
        .args((!fetch).then_some("--no-fetch"))
        .output()
        .map_err(|e| format!("could not run cargo audit: {e}"))?;
    // it exits with an error when it finds advisories: its report says whether it ran
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        format!("cargo audit didn't run (`cargo install cargo-audit --locked`): {}", String::from_utf8_lossy(&output.stderr).trim())
    })?;
    let list = report["vulnerabilities"]["list"].as_array().cloned().unwrap_or_default();
    Ok(list
        .iter()
        .filter_map(|found| Some((found["advisory"]["id"].as_str()?.to_string(), found["package"]["name"].as_str()?.to_string())))
        .collect())
}

/// The names of the workspace's direct dependencies.
fn direct(root: &Path) -> Result<BTreeSet<String>> {
    let output = cargo().current_dir(root).args(["metadata", "--no-deps", "--format-version", "1"]).output().map_err(|e| e.to_string())?;
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let packages = metadata["packages"].as_array().cloned().unwrap_or_default();
    Ok(packages
        .iter()
        .flat_map(|package| package["dependencies"].as_array().cloned().unwrap_or_default())
        .filter_map(|dependency| dependency["name"].as_str().map(str::to_string))
        .collect())
}

/// `names` as `a, b and c`.
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The changelog lines of the dependency updates from `from` to `to` (commits): a **Security** one
/// naming the advisories they fix, and a **Dependencies** one for the others. None when the app's
/// dependencies are the same.
pub fn changelog_lines(root: &Path, from: &str, to: &str) -> Result<Vec<String>> {
    let (old, new) = (git(root, &["show", &format!("{from}:Cargo.lock")])?, git(root, &["show", &format!("{to}:Cargo.lock")])?);
    let (before, after) = (packages(&old), packages(&new));
    let names: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let changed: BTreeSet<String> = names.into_iter().filter(|name| before.get(*name) != after.get(*name)).cloned().collect();
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    let dir = root.join("target/deps");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (old_path, new_path) = (dir.join("before.lock"), dir.join("after.lock"));
    std::fs::write(&old_path, old).map_err(|e| e.to_string())?;
    std::fs::write(&new_path, new).map_err(|e| e.to_string())?;
    let mut fixed = advisories(root, &old_path, true)?;
    let left = advisories(root, &new_path, false)?;
    fixed.retain(|id, _| !left.contains_key(id));

    let mut lines = Vec::new();
    let secured: BTreeSet<String> = fixed.values().cloned().collect();
    if !fixed.is_empty() {
        let ids: Vec<String> = fixed.keys().cloned().collect();
        let shown = if ids.len() > 3 { format!("{} and {} more", ids[..3].join(", "), ids.len() - 3) } else { listed(&ids) };
        let crates: Vec<String> = secured.iter().cloned().collect();
        lines.push(format!("- **Security**: {} updated, fixing {shown}", listed(&crates)));
    }
    let others: Vec<&String> = changed.iter().filter(|name| !secured.contains(*name) && before.contains_key(*name)).collect();
    if !others.is_empty() {
        // the ones the app names in its manifests first, as the best known
        let direct = direct(root)?;
        let mut named: Vec<String> = others.iter().filter(|name| direct.contains(**name)).map(|name| name.to_string()).collect();
        named.extend(others.iter().filter(|name| !direct.contains(**name)).map(|name| name.to_string()));
        let line = match named.len() {
            0..=3 => listed(&named),
            n => format!("{} and {} more", named[..3].join(", "), n - 3),
        };
        lines.push(format!("- **Dependencies**: the libraries it's built on are updated ({line})"));
    }
    Ok(lines)
}
