//! Release versions: worked out by the Release workflow from the latest `v<version>` tag and a
//! request (`cargo xtask release`), then written into the checkout being built, never committed.

use semver::{Prerelease, Version};

use std::path::Path;

use crate::tools::{Result, git, repo_root};

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

/// `lock` with the app's own `[[package]]` entry at `version`, keeping each line's own ending.
fn with_locked_version(lock: &str, version: &Version) -> Result<String> {
    let mut out = String::with_capacity(lock.len());
    let (mut in_app, mut found) = (false, false);
    for text in lock.split_inclusive('\n') {
        let line = text.trim_end();
        if in_app && line.starts_with("version = \"") {
            out += &format!("version = \"{version}\"{}", &text[line.len()..]);
            (in_app, found) = (false, true);
            continue;
        }
        in_app = line == "name = \"cachemire\"";
        out += text;
    }
    found.then_some(out).ok_or_else(|| "no cachemire package in Cargo.lock".into())
}

/// Writes `version` into Cargo.toml and Cargo.lock, building nothing.
fn write_version(root: &Path, version: &Version) -> Result<()> {
    let read = |name: &str| std::fs::read_to_string(root.join(name)).map_err(|e| format!("reading {name}: {e}"));
    let write = |name: &str, text: String| std::fs::write(root.join(name), text).map_err(|e| format!("writing {name}: {e}"));
    let toml = read("Cargo.toml")?;
    let (line, _) = find_version(&toml)?;
    let lock = with_locked_version(&read("Cargo.lock")?, version)?;
    write("Cargo.toml", with_version(&toml, line, version))?;
    write("Cargo.lock", lock)
}

/// Cargo.toml and Cargo.lock written at a release's version, put back as committed when dropped.
struct Stamped<'a> {
    root: &'a Path,
}

impl<'a> Stamped<'a> {
    fn new(root: &'a Path, version: &Version) -> Result<Self> {
        if !git(root, &["status", "--porcelain", "--", "Cargo.toml", "Cargo.lock"])?.is_empty() {
            return Err("Cargo.toml or Cargo.lock has uncommitted changes: commit or stash them first".into());
        }
        write_version(root, version)?;
        println!("Building as version {version}.");
        Ok(Self { root })
    }
}

impl Drop for Stamped<'_> {
    fn drop(&mut self) {
        if let Err(e) = git(self.root, &["checkout", "--quiet", "--", "Cargo.toml", "Cargo.lock"]) {
            eprintln!("could not put Cargo.toml and Cargo.lock back: {e}");
        }
    }
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

/// Runs `build` on a release, with its version written into Cargo.toml meanwhile. `as_version` builds the
/// working tree as that version (what the Release workflow does); otherwise the tag on HEAD if any, else the
/// latest tag (the latest stable one with `stable`), checked out meanwhile. `fast` builds the working tree as is.
pub fn in_release(fast: bool, stable: bool, as_version: Option<&str>, build: impl FnOnce() -> Result<()>) -> Result<()> {
    let root = repo_root();
    if let Some(version) = as_version {
        if fast || stable {
            return Err("--as-version builds the working tree in the release profile: it can't go with --fast or --stable".into());
        }
        let version = Version::parse(version).map_err(|e| format!("{version} is not a version: {e}"))?;
        let _stamped = Stamped::new(&root, &version)?;
        return build();
    }
    if fast {
        return if stable { Err("--fast builds the working tree: it can't go with --stable".into()) } else { build() };
    }
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
        let _stamped = Stamped::new(&root, &version)?;
        return build();
    }
    let origin = git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).or_else(|_| git(&root, &["rev-parse", "HEAD"]))?;
    println!("Checking out {tag}...");
    git(&root, &["checkout", "--quiet", &tag])?;
    // dropped in reverse: the files are put back before HEAD is
    let _restore = Restore { root: &root, origin };
    let _stamped = Stamped::new(&root, &version)?;
    build()
}

/// What a `release/<request>` branch asks for: a [`Part`], with `-stable` for `release --stable`.
fn parse_request(request: &str) -> Result<(Part, bool)> {
    let (part, stable_now) = request.strip_suffix("-stable").map_or((request, false), |part| (part, true));
    let part = <Part as clap::ValueEnum>::from_str(part, true)
        .map_err(|_| format!("`{request}` is not a release request: beta, stable, fix, minor or major, the last three optionally followed by -stable"))?;
    Ok((part, stable_now))
}

/// The request `release` pushes for `part` (and `--stable`), as its branch names it.
fn request_of(part: Part, stable_now: bool) -> String {
    let name = clap::ValueEnum::to_possible_value(&part).map_or_else(String::new, |value| value.get_name().to_string());
    if stable_now { format!("{name}-stable") } else { name }
}

/// The version `request` releases, after the highest `v*` tag of the repository.
fn next_version(root: &Path, request: &str) -> Result<Version> {
    let (part, stable_now) = parse_request(request)?;
    let last = highest(&git(root, &["tag", "--list", "v*"])?, false).ok_or("no release tag to start from (are the tags fetched?)")?;
    target_version(&last, part, stable_now)
}

/// `cargo xtask next-version`: prints the version `request` releases.
pub fn print_next_version(request: &str) -> Result<()> {
    println!("{}", next_version(&repo_root(), request)?);
    Ok(())
}

/// The `owner/repo` of the `origin` remote, when it is on GitHub.
fn github_repo(root: &Path) -> Option<String> {
    let url = git(root, &["remote", "get-url", "origin"]).ok()?;
    let path = url.split_once("github.com").map(|(_, path)| path.trim_start_matches([':', '/']))?;
    Some(path.trim_end_matches('/').trim_end_matches(".git").to_string())
}

/// `cargo xtask release`: asks the Release workflow to release HEAD, by pushing it as the
/// `release/<request>` branch. The workflow works out the version, builds, tags and publishes.
pub fn request_release(part: Part, stable_now: bool) -> Result<()> {
    let root = repo_root();
    let request = request_of(part, stable_now);
    git(&root, &["fetch", "--quiet", "--tags", "origin"])?;
    let version = next_version(&root, &request)?;
    if !git(&root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        println!("Note: the uncommitted changes are not part of the release, only HEAD is.");
    }
    let head = git(&root, &["log", "-1", "--format=%h %s"])?;
    println!("Requesting the release of {version} from {head}...");
    let branch = format!("release/{request}");
    git(&root, &["push", "--quiet", "origin", &format!("HEAD:refs/heads/{branch}")]).map_err(|e| {
        format!("{e}\nIf {branch} is already on origin, a release is under way, or one ended without removing it: wait, or remove it with `git push origin --delete {branch}`.")
    })?;
    let runs = github_repo(&root).map_or_else(|| "the repository's Actions tab".to_string(), |repo| format!("https://github.com/{repo}/actions"));
    println!("Requested. The Release workflow tags and publishes v{version} once its tests and builds pass: {runs}");
    Ok(())
}
