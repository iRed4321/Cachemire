//! `cargo bump`: releases the next version after the latest `v<version>` tag (major, minor or fix start
//! a `-beta.1`; beta and stable follow it): commits and tags it, then restores the 0.0.0-dev placeholder.

use semver::{Prerelease, Version};

use std::path::Path;

use crate::tools::{Result, cargo, git, repo_root, run};

/// The package's `version = "..."` line (the first one at the start of a line,
/// as dependency versions are indented or inline) and its value.
fn find_version(toml: &str) -> Result<(usize, Version)> {
    for (i, line) in toml.lines().enumerate() {
        let Some(rest) = line.strip_prefix("version = \"") else { continue };
        if let Ok(version) = Version::parse(rest.trim_end().trim_end_matches('"')) {
            return Ok((i, version));
        }
    }
    Err("no semver `version = \"x.y.z\"` line in Cargo.toml".into())
}

/// The package version in `toml`, as "x.y.z" or "x.y.z-pre".
pub fn read_version(toml: &str) -> Result<String> {
    Ok(find_version(toml)?.1.to_string())
}

/// The MSI "x.y.z" or "x.y.z.n" of a version, `n` being the beta number (a stable release has none).
pub fn msi_version(version: &str) -> String {
    let numeric = version.split(['-', '+']).next().unwrap_or(version);
    match version.split('-').nth(1).and_then(|pre| pre.strip_prefix("beta.")) {
        Some(rest) => format!("{numeric}.{}", rest.split(['-', '+', '.']).next().unwrap_or("0")),
        None => numeric.to_string(),
    }
}

/// What to do to the version.
#[derive(Clone, Copy, PartialEq, clap::ValueEnum)]
pub enum Part {
    Major,
    Minor,
    Fix,
    Beta,
    Stable,
}

/// The version Cargo.toml holds between releases: the real one lives in the git tags.
const PLACEHOLDER: &str = "0.0.0-dev";

/// The version of the latest `v<version>` tag reachable from `HEAD`.
fn last_released(root: &Path) -> Result<Version> {
    let tag = git(root, &["describe", "--tags", "--match", "v*", "--abbrev=0"]).map_err(|e| format!("no release tag to start from (are the tags fetched?): {e}"))?;
    Version::parse(tag.trim_start_matches('v')).map_err(|e| format!("the tag {tag} is not a version: {e}"))
}

/// The version to release for `part`, from the latest released one. From a stable version,
/// major/minor/fix start `-beta.1` (or the final version with `stable_now`); from a beta,
/// only beta and stable apply.
fn target_version(last: &Version, part: Part, stable_now: bool) -> Result<Version> {
    let (major, minor, patch) = (last.major, last.minor, last.patch);
    let beta = |major, minor, patch, number: u64| -> Result<Version> {
        let mut next = Version::new(major, minor, patch);
        next.pre = Prerelease::new(&format!("beta.{number}")).map_err(|e| e.to_string())?;
        Ok(next)
    };
    if stable_now && !matches!(part, Part::Major | Part::Minor | Part::Fix) {
        return Err("--stable only goes with major, minor or fix".into());
    }
    if last.pre.is_empty() {
        let (major, minor, patch) = match part {
            Part::Major => (major + 1, 0, 0),
            Part::Minor => (major, minor + 1, 0),
            Part::Fix => (major, minor, patch + 1),
            Part::Beta | Part::Stable => return Err(format!("{last} is already stable: run major, minor or fix")),
        };
        return if stable_now { Ok(Version::new(major, minor, patch)) } else { beta(major, minor, patch, 1) };
    }
    let number = last.pre.as_str().strip_prefix("beta.").and_then(|n| n.parse::<u64>().ok());
    match (part, number) {
        (Part::Beta, Some(n)) => beta(major, minor, patch, n + 1),
        (Part::Stable, _) => Ok(Version::new(major, minor, patch)),
        _ => Err(format!("{last} is a beta: run beta for the next one or stable to release it")),
    }
}

/// `toml` with the package version line `line` set to `version`, keeping each line's own ending (LF or CRLF).
fn with_version(toml: &str, line: usize, version: &Version) -> String {
    toml.split_inclusive('\n')
        .enumerate()
        .map(|(i, text)| if i == line { format!("version = \"{version}\"{}", &text[text.trim_end().len()..]) } else { text.to_string() })
        .collect()
}

/// Writes `version` into Cargo.toml and refreshes Cargo.lock.
fn write_version(root: &Path, line: usize, version: &Version) -> Result<()> {
    let path = root.join("Cargo.toml");
    let toml = std::fs::read_to_string(&path).map_err(|e| format!("reading Cargo.toml: {e}"))?;
    std::fs::write(&path, with_version(&toml, line, version)).map_err(|e| format!("writing Cargo.toml: {e}"))?;
    println!("Refreshing Cargo.lock...");
    run(cargo().current_dir(root).args(["check", "--quiet"]))
}

/// Commits Cargo.toml and Cargo.lock with `message`.
fn commit_version(root: &Path, message: &str) -> Result<()> {
    git(root, &["add", "Cargo.toml", "Cargo.lock"])?;
    git(root, &["commit", "-m", message, "--", "Cargo.toml", "Cargo.lock"]).map(drop)
}

/// The highest version among the `v*` tags in `tags` (one per line), skipping betas when `stable`.
fn highest(tags: &str, stable: bool) -> Option<Version> {
    let versions = tags.lines().filter_map(|tag| Version::parse(tag.trim().strip_prefix('v')?).ok());
    versions.filter(|v| !stable || v.pre.is_empty()).max()
}

/// Puts HEAD back where it was (the branch, else the commit) when dropped.
struct Restore<'a> {
    root: &'a Path,
    origin: String,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        match git(self.root, &["checkout", "--quiet", &self.origin]) {
            Ok(_) => println!("Back on {}.", self.origin),
            Err(e) => eprintln!("could not go back to {}: {e}", self.origin),
        }
    }
}

/// Runs `build` on a release: the tag on HEAD if any, else the latest tag (the latest stable one with
/// `stable`), checked out meanwhile. `fast` builds the working tree as is and can't go with `stable`.
pub fn in_release(fast: bool, stable: bool, build: impl FnOnce() -> Result<()>) -> Result<()> {
    if fast {
        return if stable { Err("--fast builds the working tree: it can't go with --stable".into()) } else { build() };
    }
    let root = repo_root();
    if !git(&root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err("the working tree has uncommitted changes: commit or stash them first".into());
    }
    let at_head = highest(&git(&root, &["tag", "--points-at", "HEAD", "--list", "v*"])?, false);
    let version = match at_head.clone() {
        Some(version) if stable && !version.pre.is_empty() => {
            println!("HEAD is on the beta tag v{version}: nothing to build with --stable.");
            return Ok(());
        }
        Some(version) => version,
        None => highest(&git(&root, &["tag", "--list", "v*"])?, stable).ok_or("no release tag to build from")?,
    };
    let tag = format!("v{version}");
    if at_head.is_some() {
        println!("Building {tag}, the tag on HEAD.");
        check_tagged_version(&root, &version)?;
        return build();
    }
    let origin = git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).or_else(|_| git(&root, &["rev-parse", "HEAD"]))?;
    println!("Checking out {tag}...");
    git(&root, &["checkout", "--quiet", &tag])?;
    let _restore = Restore { root: &root, origin };
    check_tagged_version(&root, &version)?;
    build()
}

/// Fails unless the checked out Cargo.toml holds `version`, the one of its tag.
fn check_tagged_version(root: &Path, version: &Version) -> Result<()> {
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("reading Cargo.toml: {e}"))?;
    let (_, found) = find_version(&toml)?;
    if &found != version {
        return Err(format!("the tag v{version} is on a commit whose Cargo.toml says {found}: tag releases with `cargo bump`"));
    }
    Ok(())
}

pub fn run_task(part: Part, stable_now: bool, no_tag: bool) -> Result<()> {
    let root = repo_root();
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("reading Cargo.toml: {e}"))?;
    let (line, _) = find_version(&toml)?;
    let last = last_released(&root)?;
    let new = target_version(&last, part, stable_now)?;
    let tag = format!("v{new}");
    if !no_tag {
        if !git(&root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
            return Err("the working tree has uncommitted changes: commit them first (or pass --no-tag)".into());
        }
        if !git(&root, &["tag", "--list", &tag])?.is_empty() {
            return Err(format!("the tag {tag} already exists"));
        }
    }
    println!("Bumping version: {last} -> {new}");
    write_version(&root, line, &new)?;
    if no_tag {
        println!("Version bumped to {new} (Cargo.toml, Cargo.lock).");
        return Ok(());
    }

    commit_version(&root, &format!("release {tag}"))?;
    git(&root, &["tag", "-a", &tag, "-m", &format!("Cachemire {new}")])?;
    write_version(&root, line, &Version::parse(PLACEHOLDER).map_err(|e| e.to_string())?)?;
    commit_version(&root, &format!("[AUTO] back to {PLACEHOLDER}"))?;
    println!("Released {new}: committed and tagged {tag}, then back to {PLACEHOLDER}. Build the packages from {tag}, push with `git push --follow-tags`.");
    Ok(())
}
